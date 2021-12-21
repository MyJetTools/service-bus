use std::{net::SocketAddr, sync::Arc, time::Duration};

use my_service_bus_tcp_shared::{
    ConnectionAttributes, ReadingTcpContractFail, SocketReader, TcpContract,
};
use rust_extensions::date_time::DateTimeAsMicroseconds;

use tokio::{
    io::{self, AsyncWriteExt, ReadHalf},
    net::{TcpListener, TcpStream},
};

use crate::{
    app::{logs::SystemProcess, AppContext},
    sessions::MyServiceBusSession,
};

use super::error::MySbSocketError;

pub type ConnectionId = i64;

pub async fn start(addr: SocketAddr, app: Arc<AppContext>) {
    while !app.states.is_initialized() {
        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    let listener = TcpListener::bind(addr).await.unwrap();

    tokio::spawn(super::dead_sockets_detector::start(app.clone()));

    app.logs.add_info(
        None,
        crate::app::logs::SystemProcess::TcpSocket,
        "Tcp socket is started".to_string(),
        format!("{:?}", addr),
    );

    let mut socket_id: ConnectionId = 0;

    while !app.states.is_shutting_down() {
        let accepted_socket_result = listener.accept().await;

        if let Err(err) = &accepted_socket_result {
            app.logs.add_error(
                None,
                crate::app::logs::SystemProcess::TcpSocket,
                "Accept tcp socket".to_string(),
                "Error occured".to_string(),
                Some(format!("{:?}", err)),
            );
            continue;
        }

        //Safety: We can use unwrap -since we previously checked Err status.
        let (tcp_stream, addr) = accepted_socket_result.unwrap();

        let (read_socket, mut write_socket) = io::split(tcp_stream);

        if app.states.is_shutting_down() {
            write_socket.shutdown().await.unwrap();
            break;
        }

        socket_id += 1;

        let my_sb_session = Arc::new(MyServiceBusSession::new(
            socket_id,
            format! {"{}", addr},
            write_socket,
            app.clone(),
        ));

        app.sessions.add(my_sb_session.clone()).await;

        app.logs.add_info(
            None,
            crate::app::logs::SystemProcess::TcpSocket,
            "Accepted sockets loop".to_string(),
            format!("Connected socket {}. IP: {}", my_sb_session.id, addr),
        );

        tokio::task::spawn(process_socket(read_socket, app.clone(), my_sb_session));
    }

    app.logs.add_info(
        None,
        crate::app::logs::SystemProcess::TcpSocket,
        "Tcp socket is stopped".to_string(),
        format!("{:?}", addr),
    );
}

async fn process_socket(
    read_socket: ReadHalf<TcpStream>,
    app: Arc<AppContext>,
    my_sb_session: Arc<MyServiceBusSession>,
) {
    let socket_loop_result =
        tokio::task::spawn(socket_loop(read_socket, app.clone(), my_sb_session.clone())).await;

    let name = my_sb_session.get_name().await;

    if let Err(err) = socket_loop_result {
        app.logs.add_fatal_error(
            crate::app::logs::SystemProcess::TcpSocket,
            "tcp_socket_process".to_string(),
            format!("Socket {} disconnected error: {:?}", name, err),
        );
    } else {
        app.logs.add_info(
            None,
            crate::app::logs::SystemProcess::TcpSocket,
            format!("Socket {} Processing", name),
            format!(
                "Socket with Id:{} and name {} is disconnected",
                my_sb_session.id, my_sb_session.ip
            ),
        );
    }
    app.sessions.remove(&my_sb_session.id).await;

    let on_disconnect_result = super::connection::on_disconnect(app.clone(), my_sb_session).await;

    if let Err(err) = on_disconnect_result {
        app.logs.add_fatal_error(
            SystemProcess::TcpSocket,
            "OnDisconnect Handler".to_string(),
            err,
        );
    }
}

async fn socket_loop(
    read_socket: ReadHalf<TcpStream>,
    app: Arc<AppContext>,
    session: Arc<MyServiceBusSession>,
) -> Result<(), ReadingTcpContractFail> {
    let mut socket_reader = SocketReader::new(read_socket);

    let mut attr = ConnectionAttributes::new();

    loop {
        socket_reader.start_calculating_read_size();
        let tcp_contract = TcpContract::deserialize(&mut socket_reader, &attr).await?;

        session.increase_read_size(socket_reader.read_size).await;

        let now = DateTimeAsMicroseconds::now();
        session.last_incoming_package.update(now);

        let result = super::connection::handle_incoming_payload(
            app.clone(),
            tcp_contract,
            session.clone(),
            &mut attr,
        )
        .await;

        if let Err(err) = result {
            match err {
                MySbSocketError::ReadingTcpContractFail(err) => {
                    return Err(err);
                }
                MySbSocketError::OperationFailResult(err) => {
                    let name = session.get_name().await;
                    app.logs.add_error(
                        None,
                        SystemProcess::TcpSocket,
                        "socket_loop".to_string(),
                        format!("Sending reject to the socket: {}", name),
                        Some(format!("{:?}", err)),
                    );

                    crate::operations::sessions::send_package(
                        app.as_ref(),
                        session.id,
                        TcpContract::Reject {
                            message: format!("{:?}", err),
                        },
                    )
                    .await;
                }
            }
        }
    }
}
