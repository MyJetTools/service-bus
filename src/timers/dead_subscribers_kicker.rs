use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::app::AppContext;

pub async fn start(app: Arc<AppContext>, delivery_timeout_duration: Duration) {
    let duration = Duration::from_secs(10);

    while !app.states.is_initialized() {
        tokio::time::sleep(duration).await;
    }

    println!("Kick dead subscribers timer is started");

    while !app.states.app_is_shutted_down() {
        let handler = tokio::spawn(kick_them(app.clone(), delivery_timeout_duration)).await;
        if let Err(err) = handler {
            app.logs.add_fatal_error(
                crate::app::logs::SystemProcess::Timer,
                "dead_subscribers_kicker_loop".to_string(),
                format!("{:?}", err),
            );
        }

        tokio::time::sleep(duration).await;
    }
}

pub async fn kick_them(app: Arc<AppContext>, delivery_timeout_duration: Duration) {
    let join_handle = tokio::spawn(execute(app.clone(), delivery_timeout_duration)).await;

    if let Err(err) = join_handle {
        app.logs.add_fatal_error(
            crate::app::logs::SystemProcess::Timer,
            "dead_subscribers_kicker".to_string(),
            format!("{:?}", err),
        );
    }
}

async fn execute(app: Arc<AppContext>, delivery_timeout_duration: Duration) {
    let topics = app.topic_list.get_all().await;

    let mut kicked_connections = HashMap::new();

    for topic in topics {
        let mut topic_data = topic.data.lock().await;

        for queue in topic_data.queues.get_all_mut() {
            let dead_subscribers = queue
                .subscribers
                .find_subscribers_dead_on_delivery(delivery_timeout_duration);

            if let Some(dead_subscribers) = dead_subscribers {
                for dead_subscriber in dead_subscribers {
                    app.logs.add_error(
                        Some(topic.topic_id.to_string()),
                        crate::app::logs::SystemProcess::Timer,
                        "Dead subscribers detector".to_string(),
                        format!(
                            "Kicking Connection {} with dead subscriber {}",
                            dead_subscriber.session_id, dead_subscriber.subscriber_id
                        ),
                        Some(format!("{:?}", dead_subscriber.duration)),
                    );

                    if !kicked_connections.contains_key(&dead_subscriber.session_id) {
                        kicked_connections
                            .insert(dead_subscriber.session_id, dead_subscriber.subscriber_id);
                        crate::operations::sessions::disconnect(
                            app.as_ref(),
                            dead_subscriber.session_id,
                        )
                        .await;
                    } else {
                        let kicked = kicked_connections.get(&dead_subscriber.session_id);
                        println!("We already kicked session {} the moment we were kicking the subscriber {:?}.", dead_subscriber.session_id, kicked);
                    }
                }
            }
        }
    }
}
