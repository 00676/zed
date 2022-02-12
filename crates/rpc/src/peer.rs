use super::proto::{self, AnyTypedEnvelope, EnvelopedMessage, MessageStream, RequestMessage};
use super::Connection;
use anyhow::{anyhow, Context, Result};
use futures::stream::BoxStream;
use futures::{FutureExt as _, StreamExt};
use parking_lot::{Mutex, RwLock};
use postage::{
    barrier, mpsc,
    prelude::{Sink as _, Stream as _},
};
use smol_timeout::TimeoutExt as _;
use std::sync::atomic::Ordering::SeqCst;
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    marker::PhantomData,
    sync::{
        atomic::{self, AtomicU32},
        Arc,
    },
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ConnectionId(pub u32);

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PeerId(pub u32);

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub struct Receipt<T> {
    pub sender_id: ConnectionId,
    pub message_id: u32,
    payload_type: PhantomData<T>,
}

impl<T> Clone for Receipt<T> {
    fn clone(&self) -> Self {
        Self {
            sender_id: self.sender_id,
            message_id: self.message_id,
            payload_type: PhantomData,
        }
    }
}

impl<T> Copy for Receipt<T> {}

pub struct TypedEnvelope<T> {
    pub sender_id: ConnectionId,
    pub original_sender_id: Option<PeerId>,
    pub message_id: u32,
    pub payload: T,
}

impl<T> TypedEnvelope<T> {
    pub fn original_sender_id(&self) -> Result<PeerId> {
        self.original_sender_id
            .ok_or_else(|| anyhow!("missing original_sender_id"))
    }
}

impl<T: RequestMessage> TypedEnvelope<T> {
    pub fn receipt(&self) -> Receipt<T> {
        Receipt {
            sender_id: self.sender_id,
            message_id: self.message_id,
            payload_type: PhantomData,
        }
    }
}

pub struct Peer {
    pub connections: RwLock<HashMap<ConnectionId, ConnectionState>>,
    next_connection_id: AtomicU32,
}

#[derive(Clone)]
pub struct ConnectionState {
    outgoing_tx: futures::channel::mpsc::UnboundedSender<proto::Envelope>,
    next_message_id: Arc<AtomicU32>,
    response_channels:
        Arc<Mutex<Option<HashMap<u32, mpsc::Sender<(proto::Envelope, barrier::Sender)>>>>>,
}

const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

impl Peer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            connections: Default::default(),
            next_connection_id: Default::default(),
        })
    }

    pub async fn add_connection(
        self: &Arc<Self>,
        connection: Connection,
    ) -> (
        ConnectionId,
        impl Future<Output = anyhow::Result<()>> + Send,
        BoxStream<'static, Box<dyn AnyTypedEnvelope>>,
    ) {
        // For outgoing messages, use an unbounded channel so that application code
        // can always send messages without yielding. For incoming messages, use a
        // bounded channel so that other peers will receive backpressure if they send
        // messages faster than this peer can process them.
        let (mut incoming_tx, incoming_rx) = mpsc::channel(64);
        let (outgoing_tx, mut outgoing_rx) = futures::channel::mpsc::unbounded();

        let connection_id = ConnectionId(self.next_connection_id.fetch_add(1, SeqCst));
        let connection_state = ConnectionState {
            outgoing_tx,
            next_message_id: Default::default(),
            response_channels: Arc::new(Mutex::new(Some(Default::default()))),
        };
        let mut writer = MessageStream::new(connection.tx);
        let mut reader = MessageStream::new(connection.rx);

        let this = self.clone();
        let response_channels = connection_state.response_channels.clone();
        let handle_io = async move {
            let result = 'outer: loop {
                let read_message = reader.read_message().fuse();
                futures::pin_mut!(read_message);
                loop {
                    futures::select_biased! {
                        outgoing = outgoing_rx.next().fuse() => match outgoing {
                            Some(outgoing) => {
                                match writer.write_message(&outgoing).timeout(WRITE_TIMEOUT).await {
                                    None => break 'outer Err(anyhow!("timed out writing RPC message")),
                                    Some(Err(result)) => break 'outer Err(result).context("failed to write RPC message"),
                                    _ => {}
                                }
                            }
                            None => break 'outer Ok(()),
                        },
                        incoming = read_message => match incoming {
                            Ok(incoming) => {
                                if incoming_tx.send(incoming).await.is_err() {
                                    break 'outer Ok(());
                                }
                                break;
                            }
                            Err(error) => {
                                break 'outer Err(error).context("received invalid RPC message")
                            }
                        },
                    }
                }
            };

            response_channels.lock().take();
            this.connections.write().remove(&connection_id);
            result
        };

        let response_channels = connection_state.response_channels.clone();
        self.connections
            .write()
            .insert(connection_id, connection_state);

        let incoming_rx = incoming_rx.filter_map(move |incoming| {
            let response_channels = response_channels.clone();
            async move {
                if let Some(responding_to) = incoming.responding_to {
                    let channel = response_channels.lock().as_mut()?.remove(&responding_to);
                    if let Some(mut tx) = channel {
                        let mut requester_resumed = barrier::channel();
                        tx.send((incoming, requester_resumed.0)).await.ok();
                        // Drop response channel before awaiting on the barrier. This allows the
                        // barrier to get dropped even if the request's future is dropped before it
                        // has a chance to observe the response.
                        drop(tx);
                        requester_resumed.1.recv().await;
                    } else {
                        log::warn!("received RPC response to unknown request {}", responding_to);
                    }

                    None
                } else {
                    if let Some(envelope) = proto::build_typed_envelope(connection_id, incoming) {
                        Some(envelope)
                    } else {
                        log::error!("unable to construct a typed envelope");
                        None
                    }
                }
            }
        });
        (connection_id, handle_io, incoming_rx.boxed())
    }

    pub fn disconnect(&self, connection_id: ConnectionId) {
        self.connections.write().remove(&connection_id);
    }

    pub fn reset(&self) {
        self.connections.write().clear();
    }

    pub fn request<T: RequestMessage>(
        &self,
        receiver_id: ConnectionId,
        request: T,
    ) -> impl Future<Output = Result<T::Response>> {
        self.request_internal(None, receiver_id, request)
    }

    pub fn forward_request<T: RequestMessage>(
        &self,
        sender_id: ConnectionId,
        receiver_id: ConnectionId,
        request: T,
    ) -> impl Future<Output = Result<T::Response>> {
        self.request_internal(Some(sender_id), receiver_id, request)
    }

    pub fn request_internal<T: RequestMessage>(
        &self,
        original_sender_id: Option<ConnectionId>,
        receiver_id: ConnectionId,
        request: T,
    ) -> impl Future<Output = Result<T::Response>> {
        let (tx, mut rx) = mpsc::channel(1);
        let send = self.connection_state(receiver_id).and_then(|connection| {
            let message_id = connection.next_message_id.fetch_add(1, SeqCst);
            connection
                .response_channels
                .lock()
                .as_mut()
                .ok_or_else(|| anyhow!("connection was closed"))?
                .insert(message_id, tx);
            connection
                .outgoing_tx
                .unbounded_send(request.into_envelope(
                    message_id,
                    None,
                    original_sender_id.map(|id| id.0),
                ))
                .map_err(|_| anyhow!("connection was closed"))?;
            Ok(())
        });
        async move {
            send?;
            let (response, _barrier) = rx
                .recv()
                .await
                .ok_or_else(|| anyhow!("connection was closed"))?;
            if let Some(proto::envelope::Payload::Error(error)) = &response.payload {
                Err(anyhow!("request failed").context(error.message.clone()))
            } else {
                T::Response::from_envelope(response)
                    .ok_or_else(|| anyhow!("received response of the wrong type"))
            }
        }
    }

    pub fn send<T: EnvelopedMessage>(&self, receiver_id: ConnectionId, message: T) -> Result<()> {
        let connection = self.connection_state(receiver_id)?;
        let message_id = connection
            .next_message_id
            .fetch_add(1, atomic::Ordering::SeqCst);
        connection
            .outgoing_tx
            .unbounded_send(message.into_envelope(message_id, None, None))?;
        Ok(())
    }

    pub fn forward_send<T: EnvelopedMessage>(
        &self,
        sender_id: ConnectionId,
        receiver_id: ConnectionId,
        message: T,
    ) -> Result<()> {
        let connection = self.connection_state(receiver_id)?;
        let message_id = connection
            .next_message_id
            .fetch_add(1, atomic::Ordering::SeqCst);
        connection
            .outgoing_tx
            .unbounded_send(message.into_envelope(message_id, None, Some(sender_id.0)))?;
        Ok(())
    }

    pub fn respond<T: RequestMessage>(
        &self,
        receipt: Receipt<T>,
        response: T::Response,
    ) -> Result<()> {
        let connection = self.connection_state(receipt.sender_id)?;
        let message_id = connection
            .next_message_id
            .fetch_add(1, atomic::Ordering::SeqCst);
        connection
            .outgoing_tx
            .unbounded_send(response.into_envelope(message_id, Some(receipt.message_id), None))?;
        Ok(())
    }

    pub fn respond_with_error<T: RequestMessage>(
        &self,
        receipt: Receipt<T>,
        response: proto::Error,
    ) -> Result<()> {
        let connection = self.connection_state(receipt.sender_id)?;
        let message_id = connection
            .next_message_id
            .fetch_add(1, atomic::Ordering::SeqCst);
        connection
            .outgoing_tx
            .unbounded_send(response.into_envelope(message_id, Some(receipt.message_id), None))?;
        Ok(())
    }

    fn connection_state(&self, connection_id: ConnectionId) -> Result<ConnectionState> {
        let connections = self.connections.read();
        let connection = connections
            .get(&connection_id)
            .ok_or_else(|| anyhow!("no such connection: {}", connection_id))?;
        Ok(connection.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypedEnvelope;
    use async_tungstenite::tungstenite::Message as WebSocketMessage;
    use gpui::TestAppContext;

    #[gpui::test(iterations = 50)]
    async fn test_request_response(cx: TestAppContext) {
        let executor = cx.foreground();

        // create 2 clients connected to 1 server
        let server = Peer::new();
        let client1 = Peer::new();
        let client2 = Peer::new();

        let (client1_to_server_conn, server_to_client_1_conn, _) =
            Connection::in_memory(cx.background());
        let (client1_conn_id, io_task1, client1_incoming) =
            client1.add_connection(client1_to_server_conn).await;
        let (_, io_task2, server_incoming1) = server.add_connection(server_to_client_1_conn).await;

        let (client2_to_server_conn, server_to_client_2_conn, _) =
            Connection::in_memory(cx.background());
        let (client2_conn_id, io_task3, client2_incoming) =
            client2.add_connection(client2_to_server_conn).await;
        let (_, io_task4, server_incoming2) = server.add_connection(server_to_client_2_conn).await;

        executor.spawn(io_task1).detach();
        executor.spawn(io_task2).detach();
        executor.spawn(io_task3).detach();
        executor.spawn(io_task4).detach();
        executor
            .spawn(handle_messages(server_incoming1, server.clone()))
            .detach();
        executor
            .spawn(handle_messages(client1_incoming, client1.clone()))
            .detach();
        executor
            .spawn(handle_messages(server_incoming2, server.clone()))
            .detach();
        executor
            .spawn(handle_messages(client2_incoming, client2.clone()))
            .detach();

        assert_eq!(
            client1
                .request(client1_conn_id, proto::Ping {},)
                .await
                .unwrap(),
            proto::Ack {}
        );

        assert_eq!(
            client2
                .request(client2_conn_id, proto::Ping {},)
                .await
                .unwrap(),
            proto::Ack {}
        );

        assert_eq!(
            client1
                .request(
                    client1_conn_id,
                    proto::OpenBuffer {
                        project_id: 0,
                        worktree_id: 1,
                        path: "path/one".to_string(),
                    },
                )
                .await
                .unwrap(),
            proto::OpenBufferResponse {
                buffer: Some(proto::Buffer {
                    variant: Some(proto::buffer::Variant::Id(0))
                }),
            }
        );

        assert_eq!(
            client2
                .request(
                    client2_conn_id,
                    proto::OpenBuffer {
                        project_id: 0,
                        worktree_id: 2,
                        path: "path/two".to_string(),
                    },
                )
                .await
                .unwrap(),
            proto::OpenBufferResponse {
                buffer: Some(proto::Buffer {
                    variant: Some(proto::buffer::Variant::Id(1))
                })
            }
        );

        client1.disconnect(client1_conn_id);
        client2.disconnect(client1_conn_id);

        async fn handle_messages(
            mut messages: BoxStream<'static, Box<dyn AnyTypedEnvelope>>,
            peer: Arc<Peer>,
        ) -> Result<()> {
            while let Some(envelope) = messages.next().await {
                let envelope = envelope.into_any();
                if let Some(envelope) = envelope.downcast_ref::<TypedEnvelope<proto::Ping>>() {
                    let receipt = envelope.receipt();
                    peer.respond(receipt, proto::Ack {})?
                } else if let Some(envelope) =
                    envelope.downcast_ref::<TypedEnvelope<proto::OpenBuffer>>()
                {
                    let message = &envelope.payload;
                    let receipt = envelope.receipt();
                    let response = match message.path.as_str() {
                        "path/one" => {
                            assert_eq!(message.worktree_id, 1);
                            proto::OpenBufferResponse {
                                buffer: Some(proto::Buffer {
                                    variant: Some(proto::buffer::Variant::Id(0)),
                                }),
                            }
                        }
                        "path/two" => {
                            assert_eq!(message.worktree_id, 2);
                            proto::OpenBufferResponse {
                                buffer: Some(proto::Buffer {
                                    variant: Some(proto::buffer::Variant::Id(1)),
                                }),
                            }
                        }
                        _ => {
                            panic!("unexpected path {}", message.path);
                        }
                    };

                    peer.respond(receipt, response)?
                } else {
                    panic!("unknown message type");
                }
            }

            Ok(())
        }
    }

    #[gpui::test(iterations = 50)]
    async fn test_order_of_response_and_incoming(cx: TestAppContext) {
        let executor = cx.foreground();
        let server = Peer::new();
        let client = Peer::new();

        let (client_to_server_conn, server_to_client_conn, _) =
            Connection::in_memory(cx.background());
        let (client_to_server_conn_id, io_task1, mut client_incoming) =
            client.add_connection(client_to_server_conn).await;
        let (server_to_client_conn_id, io_task2, mut server_incoming) =
            server.add_connection(server_to_client_conn).await;

        executor.spawn(io_task1).detach();
        executor.spawn(io_task2).detach();

        executor
            .spawn(async move {
                let request = server_incoming
                    .next()
                    .await
                    .unwrap()
                    .into_any()
                    .downcast::<TypedEnvelope<proto::Ping>>()
                    .unwrap();

                server
                    .send(
                        server_to_client_conn_id,
                        proto::Error {
                            message: "message 1".to_string(),
                        },
                    )
                    .unwrap();
                server
                    .send(
                        server_to_client_conn_id,
                        proto::Error {
                            message: "message 2".to_string(),
                        },
                    )
                    .unwrap();
                server.respond(request.receipt(), proto::Ack {}).unwrap();

                // Prevent the connection from being dropped
                server_incoming.next().await;
            })
            .detach();

        let events = Arc::new(Mutex::new(Vec::new()));

        let response = client.request(client_to_server_conn_id, proto::Ping {});
        let response_task = executor.spawn({
            let events = events.clone();
            async move {
                response.await.unwrap();
                events.lock().push("response".to_string());
            }
        });

        executor
            .spawn({
                let events = events.clone();
                async move {
                    let incoming1 = client_incoming
                        .next()
                        .await
                        .unwrap()
                        .into_any()
                        .downcast::<TypedEnvelope<proto::Error>>()
                        .unwrap();
                    events.lock().push(incoming1.payload.message);
                    let incoming2 = client_incoming
                        .next()
                        .await
                        .unwrap()
                        .into_any()
                        .downcast::<TypedEnvelope<proto::Error>>()
                        .unwrap();
                    events.lock().push(incoming2.payload.message);

                    // Prevent the connection from being dropped
                    client_incoming.next().await;
                }
            })
            .detach();

        response_task.await;
        assert_eq!(
            &*events.lock(),
            &[
                "message 1".to_string(),
                "message 2".to_string(),
                "response".to_string()
            ]
        );
    }

    #[gpui::test(iterations = 50)]
    async fn test_dropping_request_before_completion(cx: TestAppContext) {
        let executor = cx.foreground();
        let server = Peer::new();
        let client = Peer::new();

        let (client_to_server_conn, server_to_client_conn, _) =
            Connection::in_memory(cx.background());
        let (client_to_server_conn_id, io_task1, mut client_incoming) =
            client.add_connection(client_to_server_conn).await;
        let (server_to_client_conn_id, io_task2, mut server_incoming) =
            server.add_connection(server_to_client_conn).await;

        executor.spawn(io_task1).detach();
        executor.spawn(io_task2).detach();

        executor
            .spawn(async move {
                let request1 = server_incoming
                    .next()
                    .await
                    .unwrap()
                    .into_any()
                    .downcast::<TypedEnvelope<proto::Ping>>()
                    .unwrap();
                let request2 = server_incoming
                    .next()
                    .await
                    .unwrap()
                    .into_any()
                    .downcast::<TypedEnvelope<proto::Ping>>()
                    .unwrap();

                server
                    .send(
                        server_to_client_conn_id,
                        proto::Error {
                            message: "message 1".to_string(),
                        },
                    )
                    .unwrap();
                server
                    .send(
                        server_to_client_conn_id,
                        proto::Error {
                            message: "message 2".to_string(),
                        },
                    )
                    .unwrap();
                server.respond(request1.receipt(), proto::Ack {}).unwrap();
                server.respond(request2.receipt(), proto::Ack {}).unwrap();

                // Prevent the connection from being dropped
                server_incoming.next().await;
            })
            .detach();

        let events = Arc::new(Mutex::new(Vec::new()));

        let request1 = client.request(client_to_server_conn_id, proto::Ping {});
        let request1_task = executor.spawn(request1);
        let request2 = client.request(client_to_server_conn_id, proto::Ping {});
        let request2_task = executor.spawn({
            let events = events.clone();
            async move {
                request2.await.unwrap();
                events.lock().push("response 2".to_string());
            }
        });

        executor
            .spawn({
                let events = events.clone();
                async move {
                    let incoming1 = client_incoming
                        .next()
                        .await
                        .unwrap()
                        .into_any()
                        .downcast::<TypedEnvelope<proto::Error>>()
                        .unwrap();
                    events.lock().push(incoming1.payload.message);
                    let incoming2 = client_incoming
                        .next()
                        .await
                        .unwrap()
                        .into_any()
                        .downcast::<TypedEnvelope<proto::Error>>()
                        .unwrap();
                    events.lock().push(incoming2.payload.message);

                    // Prevent the connection from being dropped
                    client_incoming.next().await;
                }
            })
            .detach();

        // Allow the request to make some progress before dropping it.
        cx.background().simulate_random_delay().await;
        drop(request1_task);

        request2_task.await;
        assert_eq!(
            &*events.lock(),
            &[
                "message 1".to_string(),
                "message 2".to_string(),
                "response 2".to_string()
            ]
        );
    }

    #[gpui::test(iterations = 50)]
    async fn test_disconnect(cx: TestAppContext) {
        let executor = cx.foreground();

        let (client_conn, mut server_conn, _) = Connection::in_memory(cx.background());

        let client = Peer::new();
        let (connection_id, io_handler, mut incoming) = client.add_connection(client_conn).await;

        let (mut io_ended_tx, mut io_ended_rx) = postage::barrier::channel();
        executor
            .spawn(async move {
                io_handler.await.ok();
                io_ended_tx.send(()).await.unwrap();
            })
            .detach();

        let (mut messages_ended_tx, mut messages_ended_rx) = postage::barrier::channel();
        executor
            .spawn(async move {
                incoming.next().await;
                messages_ended_tx.send(()).await.unwrap();
            })
            .detach();

        client.disconnect(connection_id);

        io_ended_rx.recv().await;
        messages_ended_rx.recv().await;
        assert!(server_conn
            .send(WebSocketMessage::Binary(vec![]))
            .await
            .is_err());
    }

    #[gpui::test(iterations = 50)]
    async fn test_io_error(cx: TestAppContext) {
        let executor = cx.foreground();
        let (client_conn, mut server_conn, _) = Connection::in_memory(cx.background());

        let client = Peer::new();
        let (connection_id, io_handler, mut incoming) = client.add_connection(client_conn).await;
        executor.spawn(io_handler).detach();
        executor
            .spawn(async move { incoming.next().await })
            .detach();

        let response = executor.spawn(client.request(connection_id, proto::Ping {}));
        let _request = server_conn.rx.next().await.unwrap().unwrap();

        drop(server_conn);
        assert_eq!(
            response.await.unwrap_err().to_string(),
            "connection was closed"
        );
    }
}
