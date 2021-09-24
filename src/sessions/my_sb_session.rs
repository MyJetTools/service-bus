use std::{collections::HashMap, sync::Arc};

use my_service_bus_tcp_shared::TcpContract;
use tokio::{io::WriteHalf, net::TcpStream, sync::RwLock};

use crate::{
    app::AppContext,
    date_time::{AtomicDateTime, MyDateTime},
    operations::OperationFailResult,
    subscribers::SubscriberId,
};

use super::{MySbSessionSubscriberData, MyServiceBusSessionData};

pub type ConnectionId = i64;

pub struct MyServiceBusSession {
    pub data: RwLock<MyServiceBusSessionData>,
    pub ip: String,
    pub id: ConnectionId,
    pub connected: MyDateTime,
    pub last_incoming_package: AtomicDateTime,

    pub app: Arc<AppContext>,
}

const BADGE_HIGHLIGHT_TIMOUT: u8 = 2;

impl MyServiceBusSession {
    pub fn new(
        id: ConnectionId,
        ip: String,
        tcp_stream: WriteHalf<TcpStream>,
        app: Arc<AppContext>,
    ) -> Self {
        let now = MyDateTime::utc_now();

        let data = MyServiceBusSessionData::new(tcp_stream, app.clone());

        Self {
            id,
            ip,
            data: RwLock::new(data),
            connected: now,
            last_incoming_package: AtomicDateTime::from_date_time(now),
            app,
        }
    }

    pub async fn increase_read_size(&self, process_id: i64, read_size: usize) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].increase_read_size", self.id),
            )
            .await;
        let mut data = self.data.write().await;
        data.statistic.increase_read_size(read_size).await;

        self.app.exit_lock(process_id).await;
    }

    pub async fn set_socket_name(
        &self,
        process_id: i64,
        set_socket_name: String,
        client_version: Option<String>,
    ) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].set_socket_name", self.id),
            )
            .await;

        let mut data = self.data.write().await;
        data.name = Some(set_socket_name);
        data.client_version = client_version;

        self.app.exit_lock(process_id).await;
    }

    pub async fn set_protocol_version(&self, process_id: i64, protocol_version: i32) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].set_protocol_version", self.id),
            )
            .await;

        let mut data = self.data.write().await;
        data.attr.protocol_version = protocol_version;

        self.app.exit_lock(process_id).await;
    }

    pub async fn update_packet_versions(
        &self,
        process_id: i64,
        packet_versions: &HashMap<u8, i32>,
    ) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].update_packet_versions", self.id),
            )
            .await;
        let mut data = self.data.write().await;
        data.attr.versions.update(packet_versions);
        self.app.exit_lock(process_id).await;
    }

    pub async fn one_second_tick(&self, process_id: i64) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].one_second_tick", self.id),
            )
            .await;
        let mut write_access = self.data.write().await;
        write_access.statistic.one_second_tick();
        self.app.exit_lock(process_id).await;
    }

    pub async fn get_name(&self, process_id: i64) -> String {
        self.app
            .enter_lock(process_id, format!("MySbSession[{}].get_name", self.id))
            .await;

        let data = self.data.read().await;

        let result = match &data.name {
            Some(name) => format!("{} {}", name, self.ip),
            None => self.ip.clone(),
        };

        self.app.exit_lock(process_id).await;

        result
    }

    async fn serialize_tcp_contract(&self, tcp_contract: TcpContract) -> Vec<u8> {
        let data = self.data.read().await;
        tcp_contract.serialize(&data.attr)
    }

    async fn send_and_hadle_error(
        &self,
        process_id: i64,
        buf: &[u8],
        set_on_delivery: Option<SubscriberId>,
    ) -> bool {
        self.app
            .enter_lock(process_id, format!("MySbSession[{}].send", self.id))
            .await;

        let mut write_access = self.data.write().await;
        let result = write_access.send(buf).await;

        if let Some(subscriber_id) = set_on_delivery {
            write_access.set_on_delivery_flag(subscriber_id);
        }

        self.app.exit_lock(process_id).await;

        if let Err(err) = result {
            if write_access.logged_send_error_on_disconnected < 5 {
                self.app
                    .logs
                    .add_error(
                        None,
                        crate::app::logs::SystemProcess::TcpSocket,
                        format!("Send data to socket {:?}", write_access.name),
                        format!("Can not send data to the socket {:?}", write_access.name),
                        Some(err),
                    )
                    .await;
            }

            write_access.logged_send_error_on_disconnected += 1;

            return false;
        }

        true
    }

    pub async fn send(&self, process_id: i64, tcp_contract: TcpContract) {
        let buf = self.serialize_tcp_contract(tcp_contract).await;

        if !self
            .send_and_hadle_error(process_id, buf.as_slice(), None)
            .await
        {
            self.disconnect(process_id).await;
        }
    }

    pub async fn send_and_set_on_delivery(
        &self,
        process_id: i64,
        tcp_contract: TcpContract,
        subscriber_id: SubscriberId,
    ) {
        let buf = self.serialize_tcp_contract(tcp_contract).await;

        if !self
            .send_and_hadle_error(process_id, buf.as_slice(), Some(subscriber_id))
            .await
        {
            self.disconnect(process_id).await;
        }
    }

    pub async fn add_publisher(&self, process_id: i64, topic: &str) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].add_publisher", self.id),
            )
            .await;
        let mut data = self.data.write().await;

        data.statistic
            .publishers
            .insert(topic.to_string(), BADGE_HIGHLIGHT_TIMOUT);

        if !data.statistic.publishers.contains_key(topic) {
            data.statistic
                .publishers
                .insert(topic.to_string(), BADGE_HIGHLIGHT_TIMOUT);
        }

        self.app.exit_lock(process_id).await;
    }

    pub async fn add_subscriber(
        &self,
        process_id: i64,
        subscriber_id: SubscriberId,
        topic_id: &str,
        queue_id: &str,
    ) -> Result<(), OperationFailResult> {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].add_subscriber", self.id),
            )
            .await;

        let mut data = self.data.write().await;
        if data.is_disconnected() {
            return Err(OperationFailResult::SessionIsDisconnected);
        }
        data.statistic.subscribers.insert(
            subscriber_id,
            MySbSessionSubscriberData::new(topic_id, queue_id, 0),
        );

        data.add_subscriber(&subscriber_id, topic_id, queue_id);

        self.app.exit_lock(process_id).await;
        return Ok(());
    }

    pub async fn set_delivered_statistic(
        &self,
        process_id: i64,
        subscriber_id: i64,
        delivered: usize,
        microseconds: usize,
    ) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].set_delivered_statistic", self.id),
            )
            .await;

        let mut write_access = self.data.write().await;

        let found_subscriber = write_access.statistic.subscribers.get_mut(&subscriber_id);

        if let Some(subscriber) = found_subscriber {
            subscriber.delivered_amount.increase(delivered);
            subscriber.delivery_microseconds.increase(microseconds);
        }

        self.app.exit_lock(process_id).await;
    }

    pub async fn set_not_delivered_statistic(
        &self,
        process_id: i64,
        subscriber_id: i64,
        delivered: i32,
        microseconds: i32,
    ) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].set_not_delivered_statistic", self.id),
            )
            .await;

        let mut write_access = self.data.write().await;

        let found_subscriber = write_access.statistic.subscribers.get_mut(&subscriber_id);

        if let Some(subscriber) = found_subscriber {
            subscriber.metrics.put(microseconds / -delivered)
        }

        self.app.exit_lock(process_id).await;
    }

    pub async fn remove_subscriber(&self, process_id: i64, subscriber_id: SubscriberId) {
        self.app
            .enter_lock(
                process_id,
                format!("MySbSession[{}].remove_subscriber", self.id),
            )
            .await;
        let mut statistic_write_access = self.data.write().await;
        statistic_write_access
            .statistic
            .subscribers
            .remove(&subscriber_id);

        statistic_write_access.remove_subscriber(&subscriber_id);
        self.app.exit_lock(process_id).await;
    }

    pub async fn disconnect(
        &self,
        process_id: i64,
    ) -> Option<HashMap<SubscriberId, MySbSessionSubscriberData>> {
        self.app
            .enter_lock(process_id, format!("MySbSession[{}].disconnect", self.id))
            .await;

        let mut write_access = self.data.write().await;

        write_access.disconnect().await;

        self.app.exit_lock(process_id).await;
        return Some(write_access.get_subscribers());
    }
}
