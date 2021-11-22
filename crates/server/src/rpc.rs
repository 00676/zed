mod store;

use super::{
    auth::process_auth_header,
    db::{ChannelId, MessageId, UserId},
    AppState,
};
use anyhow::anyhow;
use async_std::task;
use async_tungstenite::{tungstenite::protocol::Role, WebSocketStream};
use futures::{future::BoxFuture, FutureExt};
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use postage::{mpsc, prelude::Sink as _, prelude::Stream as _};
use rpc::{
    proto::{self, AnyTypedEnvelope, EnvelopedMessage},
    Connection, ConnectionId, Peer, TypedEnvelope,
};
use sha1::{Digest as _, Sha1};
use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    future::Future,
    mem,
    sync::Arc,
    time::Instant,
};
use store::{Store, Worktree};
use surf::StatusCode;
use tide::log;
use tide::{
    http::headers::{HeaderName, CONNECTION, UPGRADE},
    Request, Response,
};
use time::OffsetDateTime;

type MessageHandler = Box<
    dyn Send
        + Sync
        + Fn(Arc<Server>, Box<dyn AnyTypedEnvelope>) -> BoxFuture<'static, tide::Result<()>>,
>;

pub struct Server {
    peer: Arc<Peer>,
    store: RwLock<Store>,
    app_state: Arc<AppState>,
    handlers: HashMap<TypeId, MessageHandler>,
    notifications: Option<mpsc::Sender<()>>,
}

const MESSAGE_COUNT_PER_PAGE: usize = 100;
const MAX_MESSAGE_LEN: usize = 1024;

impl Server {
    pub fn new(
        app_state: Arc<AppState>,
        peer: Arc<Peer>,
        notifications: Option<mpsc::Sender<()>>,
    ) -> Arc<Self> {
        let mut server = Self {
            peer,
            app_state,
            store: Default::default(),
            handlers: Default::default(),
            notifications,
        };

        server
            .add_handler(Server::ping)
            .add_handler(Server::open_worktree)
            .add_handler(Server::close_worktree)
            .add_handler(Server::share_worktree)
            .add_handler(Server::unshare_worktree)
            .add_handler(Server::join_worktree)
            .add_handler(Server::leave_worktree)
            .add_handler(Server::update_worktree)
            .add_handler(Server::open_buffer)
            .add_handler(Server::close_buffer)
            .add_handler(Server::update_buffer)
            .add_handler(Server::buffer_saved)
            .add_handler(Server::save_buffer)
            .add_handler(Server::get_channels)
            .add_handler(Server::get_users)
            .add_handler(Server::join_channel)
            .add_handler(Server::leave_channel)
            .add_handler(Server::send_channel_message)
            .add_handler(Server::get_channel_messages);

        Arc::new(server)
    }

    fn add_handler<F, Fut, M>(&mut self, handler: F) -> &mut Self
    where
        F: 'static + Send + Sync + Fn(Arc<Self>, TypedEnvelope<M>) -> Fut,
        Fut: 'static + Send + Future<Output = tide::Result<()>>,
        M: EnvelopedMessage,
    {
        let prev_handler = self.handlers.insert(
            TypeId::of::<M>(),
            Box::new(move |server, envelope| {
                let envelope = envelope.into_any().downcast::<TypedEnvelope<M>>().unwrap();
                (handler)(server, *envelope).boxed()
            }),
        );
        if prev_handler.is_some() {
            panic!("registered a handler for the same message twice");
        }
        self
    }

    pub fn handle_connection(
        self: &Arc<Self>,
        connection: Connection,
        addr: String,
        user_id: UserId,
    ) -> impl Future<Output = ()> {
        let mut this = self.clone();
        async move {
            let (connection_id, handle_io, mut incoming_rx) =
                this.peer.add_connection(connection).await;
            this.state_mut().add_connection(connection_id, user_id);
            if let Err(err) = this.update_collaborators_for_users(&[user_id]).await {
                log::error!("error updating collaborators for {:?}: {}", user_id, err);
            }

            let handle_io = handle_io.fuse();
            futures::pin_mut!(handle_io);
            loop {
                let next_message = incoming_rx.recv().fuse();
                futures::pin_mut!(next_message);
                futures::select_biased! {
                    message = next_message => {
                        if let Some(message) = message {
                            let start_time = Instant::now();
                            log::info!("RPC message received: {}", message.payload_type_name());
                            if let Some(handler) = this.handlers.get(&message.payload_type_id()) {
                                if let Err(err) = (handler)(this.clone(), message).await {
                                    log::error!("error handling message: {:?}", err);
                                } else {
                                    log::info!("RPC message handled. duration:{:?}", start_time.elapsed());
                                }

                                if let Some(mut notifications) = this.notifications.clone() {
                                    let _ = notifications.send(()).await;
                                }
                            } else {
                                log::warn!("unhandled message: {}", message.payload_type_name());
                            }
                        } else {
                            log::info!("rpc connection closed {:?}", addr);
                            break;
                        }
                    }
                    handle_io = handle_io => {
                        if let Err(err) = handle_io {
                            log::error!("error handling rpc connection {:?} - {:?}", addr, err);
                        }
                        break;
                    }
                }
            }

            if let Err(err) = this.sign_out(connection_id).await {
                log::error!("error signing out connection {:?} - {:?}", addr, err);
            }
        }
    }

    async fn sign_out(self: &mut Arc<Self>, connection_id: ConnectionId) -> tide::Result<()> {
        self.peer.disconnect(connection_id).await;
        let removed_connection = self.state_mut().remove_connection(connection_id)?;

        for (worktree_id, worktree) in removed_connection.hosted_worktrees {
            if let Some(share) = worktree.share {
                broadcast(
                    connection_id,
                    share.guest_connection_ids.keys().copied().collect(),
                    |conn_id| {
                        self.peer
                            .send(conn_id, proto::UnshareWorktree { worktree_id })
                    },
                )
                .await?;
            }
        }

        for (worktree_id, peer_ids) in removed_connection.guest_worktree_ids {
            broadcast(connection_id, peer_ids, |conn_id| {
                self.peer.send(
                    conn_id,
                    proto::RemovePeer {
                        worktree_id,
                        peer_id: connection_id.0,
                    },
                )
            })
            .await?;
        }

        self.update_collaborators_for_users(removed_connection.collaborator_ids.iter())
            .await?;

        Ok(())
    }

    async fn ping(self: Arc<Server>, request: TypedEnvelope<proto::Ping>) -> tide::Result<()> {
        self.peer.respond(request.receipt(), proto::Ack {}).await?;
        Ok(())
    }

    async fn open_worktree(
        mut self: Arc<Server>,
        request: TypedEnvelope<proto::OpenWorktree>,
    ) -> tide::Result<()> {
        let receipt = request.receipt();
        let host_user_id = self.state().user_id_for_connection(request.sender_id)?;

        let mut collaborator_user_ids = HashSet::new();
        collaborator_user_ids.insert(host_user_id);
        for github_login in request.payload.collaborator_logins {
            match self.app_state.db.create_user(&github_login, false).await {
                Ok(collaborator_user_id) => {
                    collaborator_user_ids.insert(collaborator_user_id);
                }
                Err(err) => {
                    let message = err.to_string();
                    self.peer
                        .respond_with_error(receipt, proto::Error { message })
                        .await?;
                    return Ok(());
                }
            }
        }

        let collaborator_user_ids = collaborator_user_ids.into_iter().collect::<Vec<_>>();
        let worktree_id = self.state_mut().add_worktree(Worktree {
            host_connection_id: request.sender_id,
            collaborator_user_ids: collaborator_user_ids.clone(),
            root_name: request.payload.root_name,
            share: None,
        });

        self.peer
            .respond(receipt, proto::OpenWorktreeResponse { worktree_id })
            .await?;
        self.update_collaborators_for_users(&collaborator_user_ids)
            .await?;

        Ok(())
    }

    async fn close_worktree(
        mut self: Arc<Server>,
        request: TypedEnvelope<proto::CloseWorktree>,
    ) -> tide::Result<()> {
        let worktree_id = request.payload.worktree_id;
        let worktree = self
            .state_mut()
            .remove_worktree(worktree_id, request.sender_id)?;

        if let Some(share) = worktree.share {
            broadcast(
                request.sender_id,
                share.guest_connection_ids.keys().copied().collect(),
                |conn_id| {
                    self.peer
                        .send(conn_id, proto::UnshareWorktree { worktree_id })
                },
            )
            .await?;
        }
        self.update_collaborators_for_users(&worktree.collaborator_user_ids)
            .await?;
        Ok(())
    }

    async fn share_worktree(
        mut self: Arc<Server>,
        mut request: TypedEnvelope<proto::ShareWorktree>,
    ) -> tide::Result<()> {
        let worktree = request
            .payload
            .worktree
            .as_mut()
            .ok_or_else(|| anyhow!("missing worktree"))?;
        let entries = mem::take(&mut worktree.entries)
            .into_iter()
            .map(|entry| (entry.id, entry))
            .collect();

        let collaborator_user_ids =
            self.state_mut()
                .share_worktree(worktree.id, request.sender_id, entries);
        if let Some(collaborator_user_ids) = collaborator_user_ids {
            self.peer
                .respond(request.receipt(), proto::ShareWorktreeResponse {})
                .await?;
            self.update_collaborators_for_users(&collaborator_user_ids)
                .await?;
        } else {
            self.peer
                .respond_with_error(
                    request.receipt(),
                    proto::Error {
                        message: "no such worktree".to_string(),
                    },
                )
                .await?;
        }
        Ok(())
    }

    async fn unshare_worktree(
        mut self: Arc<Server>,
        request: TypedEnvelope<proto::UnshareWorktree>,
    ) -> tide::Result<()> {
        let worktree_id = request.payload.worktree_id;
        let worktree = self
            .state_mut()
            .unshare_worktree(worktree_id, request.sender_id)?;

        broadcast(request.sender_id, worktree.connection_ids, |conn_id| {
            self.peer
                .send(conn_id, proto::UnshareWorktree { worktree_id })
        })
        .await?;
        self.update_collaborators_for_users(&worktree.collaborator_ids)
            .await?;

        Ok(())
    }

    async fn join_worktree(
        mut self: Arc<Server>,
        request: TypedEnvelope<proto::JoinWorktree>,
    ) -> tide::Result<()> {
        let worktree_id = request.payload.worktree_id;

        let user_id = self.state().user_id_for_connection(request.sender_id)?;
        let response_data = self
            .state_mut()
            .join_worktree(request.sender_id, user_id, worktree_id)
            .and_then(|joined| {
                let share = joined.worktree.share()?;
                let peer_count = share.guest_connection_ids.len();
                let mut peers = Vec::with_capacity(peer_count);
                peers.push(proto::Peer {
                    peer_id: joined.worktree.host_connection_id.0,
                    replica_id: 0,
                });
                for (peer_conn_id, peer_replica_id) in &share.guest_connection_ids {
                    if *peer_conn_id != request.sender_id {
                        peers.push(proto::Peer {
                            peer_id: peer_conn_id.0,
                            replica_id: *peer_replica_id as u32,
                        });
                    }
                }
                let response = proto::JoinWorktreeResponse {
                    worktree: Some(proto::Worktree {
                        id: worktree_id,
                        root_name: joined.worktree.root_name.clone(),
                        entries: share.entries.values().cloned().collect(),
                    }),
                    replica_id: joined.replica_id as u32,
                    peers,
                };
                let connection_ids = joined.worktree.connection_ids();
                let collaborator_user_ids = joined.worktree.collaborator_user_ids.clone();
                Ok((response, connection_ids, collaborator_user_ids))
            });

        match response_data {
            Ok((response, connection_ids, collaborator_user_ids)) => {
                broadcast(request.sender_id, connection_ids, |conn_id| {
                    self.peer.send(
                        conn_id,
                        proto::AddPeer {
                            worktree_id,
                            peer: Some(proto::Peer {
                                peer_id: request.sender_id.0,
                                replica_id: response.replica_id,
                            }),
                        },
                    )
                })
                .await?;
                self.peer.respond(request.receipt(), response).await?;
                self.update_collaborators_for_users(&collaborator_user_ids)
                    .await?;
            }
            Err(error) => {
                self.peer
                    .respond_with_error(
                        request.receipt(),
                        proto::Error {
                            message: error.to_string(),
                        },
                    )
                    .await?;
            }
        }

        Ok(())
    }

    async fn leave_worktree(
        mut self: Arc<Server>,
        request: TypedEnvelope<proto::LeaveWorktree>,
    ) -> tide::Result<()> {
        let sender_id = request.sender_id;
        let worktree_id = request.payload.worktree_id;
        let worktree = self.state_mut().leave_worktree(sender_id, worktree_id);
        if let Some(worktree) = worktree {
            broadcast(sender_id, worktree.connection_ids, |conn_id| {
                self.peer.send(
                    conn_id,
                    proto::RemovePeer {
                        worktree_id,
                        peer_id: sender_id.0,
                    },
                )
            })
            .await?;
            self.update_collaborators_for_users(&worktree.collaborator_ids)
                .await?;
        }
        Ok(())
    }

    async fn update_worktree(
        mut self: Arc<Server>,
        request: TypedEnvelope<proto::UpdateWorktree>,
    ) -> tide::Result<()> {
        let connection_ids = self.state_mut().update_worktree(
            request.sender_id,
            request.payload.worktree_id,
            &request.payload.removed_entries,
            &request.payload.updated_entries,
        )?;

        broadcast(request.sender_id, connection_ids, |connection_id| {
            self.peer
                .forward_send(request.sender_id, connection_id, request.payload.clone())
        })
        .await?;

        Ok(())
    }

    async fn open_buffer(
        self: Arc<Server>,
        request: TypedEnvelope<proto::OpenBuffer>,
    ) -> tide::Result<()> {
        let receipt = request.receipt();
        let host_connection_id = self
            .state()
            .worktree_host_connection_id(request.sender_id, request.payload.worktree_id)?;
        let response = self
            .peer
            .forward_request(request.sender_id, host_connection_id, request.payload)
            .await?;
        self.peer.respond(receipt, response).await?;
        Ok(())
    }

    async fn close_buffer(
        self: Arc<Server>,
        request: TypedEnvelope<proto::CloseBuffer>,
    ) -> tide::Result<()> {
        let host_connection_id = self
            .state()
            .worktree_host_connection_id(request.sender_id, request.payload.worktree_id)?;
        self.peer
            .forward_send(request.sender_id, host_connection_id, request.payload)
            .await?;
        Ok(())
    }

    async fn save_buffer(
        self: Arc<Server>,
        request: TypedEnvelope<proto::SaveBuffer>,
    ) -> tide::Result<()> {
        let host;
        let guests;
        {
            let state = self.state();
            host = state
                .worktree_host_connection_id(request.sender_id, request.payload.worktree_id)?;
            guests = state
                .worktree_guest_connection_ids(request.sender_id, request.payload.worktree_id)?;
        }

        let sender = request.sender_id;
        let receipt = request.receipt();
        let response = self
            .peer
            .forward_request(sender, host, request.payload.clone())
            .await?;

        broadcast(host, guests, |conn_id| {
            let response = response.clone();
            let peer = &self.peer;
            async move {
                if conn_id == sender {
                    peer.respond(receipt, response).await
                } else {
                    peer.forward_send(host, conn_id, response).await
                }
            }
        })
        .await?;

        Ok(())
    }

    async fn update_buffer(
        self: Arc<Server>,
        request: TypedEnvelope<proto::UpdateBuffer>,
    ) -> tide::Result<()> {
        let receiver_ids = self
            .state()
            .worktree_connection_ids(request.sender_id, request.payload.worktree_id)?;
        broadcast(request.sender_id, receiver_ids, |connection_id| {
            self.peer
                .forward_send(request.sender_id, connection_id, request.payload.clone())
        })
        .await?;
        self.peer.respond(request.receipt(), proto::Ack {}).await?;
        Ok(())
    }

    async fn buffer_saved(
        self: Arc<Server>,
        request: TypedEnvelope<proto::BufferSaved>,
    ) -> tide::Result<()> {
        let receiver_ids = self
            .state()
            .worktree_connection_ids(request.sender_id, request.payload.worktree_id)?;
        broadcast(request.sender_id, receiver_ids, |connection_id| {
            self.peer
                .forward_send(request.sender_id, connection_id, request.payload.clone())
        })
        .await?;
        Ok(())
    }

    async fn get_channels(
        self: Arc<Server>,
        request: TypedEnvelope<proto::GetChannels>,
    ) -> tide::Result<()> {
        let user_id = self.state().user_id_for_connection(request.sender_id)?;
        let channels = self.app_state.db.get_accessible_channels(user_id).await?;
        self.peer
            .respond(
                request.receipt(),
                proto::GetChannelsResponse {
                    channels: channels
                        .into_iter()
                        .map(|chan| proto::Channel {
                            id: chan.id.to_proto(),
                            name: chan.name,
                        })
                        .collect(),
                },
            )
            .await?;
        Ok(())
    }

    async fn get_users(
        self: Arc<Server>,
        request: TypedEnvelope<proto::GetUsers>,
    ) -> tide::Result<()> {
        let receipt = request.receipt();
        let user_ids = request.payload.user_ids.into_iter().map(UserId::from_proto);
        let users = self
            .app_state
            .db
            .get_users_by_ids(user_ids)
            .await?
            .into_iter()
            .map(|user| proto::User {
                id: user.id.to_proto(),
                avatar_url: format!("https://github.com/{}.png?size=128", user.github_login),
                github_login: user.github_login,
            })
            .collect();
        self.peer
            .respond(receipt, proto::GetUsersResponse { users })
            .await?;
        Ok(())
    }

    async fn update_collaborators_for_users<'a>(
        self: &Arc<Server>,
        user_ids: impl IntoIterator<Item = &'a UserId>,
    ) -> tide::Result<()> {
        let mut send_futures = Vec::new();

        {
            let state = self.state();
            for user_id in user_ids {
                let collaborators = state.collaborators_for_user(*user_id);
                for connection_id in state.connection_ids_for_user(*user_id) {
                    send_futures.push(self.peer.send(
                        connection_id,
                        proto::UpdateCollaborators {
                            collaborators: collaborators.clone(),
                        },
                    ));
                }
            }
        }
        futures::future::try_join_all(send_futures).await?;

        Ok(())
    }

    async fn join_channel(
        mut self: Arc<Self>,
        request: TypedEnvelope<proto::JoinChannel>,
    ) -> tide::Result<()> {
        let user_id = self.state().user_id_for_connection(request.sender_id)?;
        let channel_id = ChannelId::from_proto(request.payload.channel_id);
        if !self
            .app_state
            .db
            .can_user_access_channel(user_id, channel_id)
            .await?
        {
            Err(anyhow!("access denied"))?;
        }

        self.state_mut().join_channel(request.sender_id, channel_id);
        let messages = self
            .app_state
            .db
            .get_channel_messages(channel_id, MESSAGE_COUNT_PER_PAGE, None)
            .await?
            .into_iter()
            .map(|msg| proto::ChannelMessage {
                id: msg.id.to_proto(),
                body: msg.body,
                timestamp: msg.sent_at.unix_timestamp() as u64,
                sender_id: msg.sender_id.to_proto(),
                nonce: Some(msg.nonce.as_u128().into()),
            })
            .collect::<Vec<_>>();
        self.peer
            .respond(
                request.receipt(),
                proto::JoinChannelResponse {
                    done: messages.len() < MESSAGE_COUNT_PER_PAGE,
                    messages,
                },
            )
            .await?;
        Ok(())
    }

    async fn leave_channel(
        mut self: Arc<Self>,
        request: TypedEnvelope<proto::LeaveChannel>,
    ) -> tide::Result<()> {
        let user_id = self.state().user_id_for_connection(request.sender_id)?;
        let channel_id = ChannelId::from_proto(request.payload.channel_id);
        if !self
            .app_state
            .db
            .can_user_access_channel(user_id, channel_id)
            .await?
        {
            Err(anyhow!("access denied"))?;
        }

        self.state_mut()
            .leave_channel(request.sender_id, channel_id);

        Ok(())
    }

    async fn send_channel_message(
        self: Arc<Self>,
        request: TypedEnvelope<proto::SendChannelMessage>,
    ) -> tide::Result<()> {
        let receipt = request.receipt();
        let channel_id = ChannelId::from_proto(request.payload.channel_id);
        let user_id;
        let connection_ids;
        {
            let state = self.state();
            user_id = state.user_id_for_connection(request.sender_id)?;
            if let Some(ids) = state.channel_connection_ids(channel_id) {
                connection_ids = ids;
            } else {
                return Ok(());
            }
        }

        // Validate the message body.
        let body = request.payload.body.trim().to_string();
        if body.len() > MAX_MESSAGE_LEN {
            self.peer
                .respond_with_error(
                    receipt,
                    proto::Error {
                        message: "message is too long".to_string(),
                    },
                )
                .await?;
            return Ok(());
        }
        if body.is_empty() {
            self.peer
                .respond_with_error(
                    receipt,
                    proto::Error {
                        message: "message can't be blank".to_string(),
                    },
                )
                .await?;
            return Ok(());
        }

        let timestamp = OffsetDateTime::now_utc();
        let nonce = if let Some(nonce) = request.payload.nonce {
            nonce
        } else {
            self.peer
                .respond_with_error(
                    receipt,
                    proto::Error {
                        message: "nonce can't be blank".to_string(),
                    },
                )
                .await?;
            return Ok(());
        };

        let message_id = self
            .app_state
            .db
            .create_channel_message(channel_id, user_id, &body, timestamp, nonce.clone().into())
            .await?
            .to_proto();
        let message = proto::ChannelMessage {
            sender_id: user_id.to_proto(),
            id: message_id,
            body,
            timestamp: timestamp.unix_timestamp() as u64,
            nonce: Some(nonce),
        };
        broadcast(request.sender_id, connection_ids, |conn_id| {
            self.peer.send(
                conn_id,
                proto::ChannelMessageSent {
                    channel_id: channel_id.to_proto(),
                    message: Some(message.clone()),
                },
            )
        })
        .await?;
        self.peer
            .respond(
                receipt,
                proto::SendChannelMessageResponse {
                    message: Some(message),
                },
            )
            .await?;
        Ok(())
    }

    async fn get_channel_messages(
        self: Arc<Self>,
        request: TypedEnvelope<proto::GetChannelMessages>,
    ) -> tide::Result<()> {
        let user_id = self.state().user_id_for_connection(request.sender_id)?;
        let channel_id = ChannelId::from_proto(request.payload.channel_id);
        if !self
            .app_state
            .db
            .can_user_access_channel(user_id, channel_id)
            .await?
        {
            Err(anyhow!("access denied"))?;
        }

        let messages = self
            .app_state
            .db
            .get_channel_messages(
                channel_id,
                MESSAGE_COUNT_PER_PAGE,
                Some(MessageId::from_proto(request.payload.before_message_id)),
            )
            .await?
            .into_iter()
            .map(|msg| proto::ChannelMessage {
                id: msg.id.to_proto(),
                body: msg.body,
                timestamp: msg.sent_at.unix_timestamp() as u64,
                sender_id: msg.sender_id.to_proto(),
                nonce: Some(msg.nonce.as_u128().into()),
            })
            .collect::<Vec<_>>();
        self.peer
            .respond(
                request.receipt(),
                proto::GetChannelMessagesResponse {
                    done: messages.len() < MESSAGE_COUNT_PER_PAGE,
                    messages,
                },
            )
            .await?;
        Ok(())
    }

    fn state<'a>(self: &'a Arc<Self>) -> RwLockReadGuard<'a, Store> {
        self.store.read()
    }

    fn state_mut<'a>(self: &'a mut Arc<Self>) -> RwLockWriteGuard<'a, Store> {
        self.store.write()
    }
}

pub async fn broadcast<F, T>(
    sender_id: ConnectionId,
    receiver_ids: Vec<ConnectionId>,
    mut f: F,
) -> anyhow::Result<()>
where
    F: FnMut(ConnectionId) -> T,
    T: Future<Output = anyhow::Result<()>>,
{
    let futures = receiver_ids
        .into_iter()
        .filter(|id| *id != sender_id)
        .map(|id| f(id));
    futures::future::try_join_all(futures).await?;
    Ok(())
}

pub fn add_routes(app: &mut tide::Server<Arc<AppState>>, rpc: &Arc<Peer>) {
    let server = Server::new(app.state().clone(), rpc.clone(), None);
    app.at("/rpc").get(move |request: Request<Arc<AppState>>| {
        let server = server.clone();
        async move {
            const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

            let connection_upgrade = header_contains_ignore_case(&request, CONNECTION, "upgrade");
            let upgrade_to_websocket = header_contains_ignore_case(&request, UPGRADE, "websocket");
            let upgrade_requested = connection_upgrade && upgrade_to_websocket;
            let client_protocol_version: Option<u32> = request
                .header("X-Zed-Protocol-Version")
                .and_then(|v| v.as_str().parse().ok());

            if !upgrade_requested || client_protocol_version != Some(rpc::PROTOCOL_VERSION) {
                return Ok(Response::new(StatusCode::UpgradeRequired));
            }

            let header = match request.header("Sec-Websocket-Key") {
                Some(h) => h.as_str(),
                None => return Err(anyhow!("expected sec-websocket-key"))?,
            };

            let user_id = process_auth_header(&request).await?;

            let mut response = Response::new(StatusCode::SwitchingProtocols);
            response.insert_header(UPGRADE, "websocket");
            response.insert_header(CONNECTION, "Upgrade");
            let hash = Sha1::new().chain(header).chain(WEBSOCKET_GUID).finalize();
            response.insert_header("Sec-Websocket-Accept", base64::encode(&hash[..]));
            response.insert_header("Sec-Websocket-Version", "13");

            let http_res: &mut tide::http::Response = response.as_mut();
            let upgrade_receiver = http_res.recv_upgrade().await;
            let addr = request.remote().unwrap_or("unknown").to_string();
            task::spawn(async move {
                if let Some(stream) = upgrade_receiver.await {
                    server
                        .handle_connection(
                            Connection::new(
                                WebSocketStream::from_raw_socket(stream, Role::Server, None).await,
                            ),
                            addr,
                            user_id,
                        )
                        .await;
                }
            });

            Ok(response)
        }
    });
}

fn header_contains_ignore_case<T>(
    request: &tide::Request<T>,
    header_name: HeaderName,
    value: &str,
) -> bool {
    request
        .header(header_name)
        .map(|h| {
            h.as_str()
                .split(',')
                .any(|s| s.trim().eq_ignore_ascii_case(value.trim()))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth,
        db::{tests::TestDb, UserId},
        github, AppState, Config,
    };
    use ::rpc::Peer;
    use async_std::task;
    use gpui::{ModelHandle, TestAppContext};
    use parking_lot::Mutex;
    use postage::{mpsc, watch};
    use serde_json::json;
    use sqlx::types::time::OffsetDateTime;
    use std::{
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering::SeqCst},
            Arc,
        },
        time::Duration,
    };
    use zed::{
        client::{
            self, test::FakeHttpClient, Channel, ChannelDetails, ChannelList, Client, Credentials,
            EstablishConnectionError, UserStore,
        },
        editor::{Editor, EditorSettings, Input},
        fs::{FakeFs, Fs as _},
        language::{
            tree_sitter_rust, Diagnostic, Language, LanguageConfig, LanguageRegistry,
            LanguageServerConfig, Point,
        },
        lsp,
        people_panel::JoinWorktree,
        project::{ProjectPath, Worktree},
        workspace::{Workspace, WorkspaceParams},
    };

    #[gpui::test]
    async fn test_share_worktree(mut cx_a: TestAppContext, mut cx_b: TestAppContext) {
        let (window_b, _) = cx_b.add_window(|_| EmptyView);
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_b, "user_b").await;

        cx_a.foreground().forbid_parking();

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/a",
            json!({
                ".zed.toml": r#"collaborators = ["user_b"]"#,
                "a.txt": "a-contents",
                "b.txt": "b-contents",
            }),
        )
        .await;
        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/a".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Join that worktree as client B, and see that a guest has joined as client A.
        let worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();
        let replica_id_b = worktree_b.read_with(&cx_b, |tree, _| tree.replica_id());
        worktree_a
            .condition(&cx_a, |tree, _| {
                tree.peers()
                    .values()
                    .any(|replica_id| *replica_id == replica_id_b)
            })
            .await;

        // Open the same file as client B and client A.
        let buffer_b = worktree_b
            .update(&mut cx_b, |worktree, cx| worktree.open_buffer("b.txt", cx))
            .await
            .unwrap();
        buffer_b.read_with(&cx_b, |buf, _| assert_eq!(buf.text(), "b-contents"));
        worktree_a.read_with(&cx_a, |tree, cx| assert!(tree.has_open_buffer("b.txt", cx)));
        let buffer_a = worktree_a
            .update(&mut cx_a, |tree, cx| tree.open_buffer("b.txt", cx))
            .await
            .unwrap();

        // Create a selection set as client B and see that selection set as client A.
        let editor_b = cx_b.add_view(window_b, |cx| {
            Editor::for_buffer(buffer_b, |cx| EditorSettings::test(cx), cx)
        });
        buffer_a
            .condition(&cx_a, |buffer, _| buffer.selection_sets().count() == 1)
            .await;

        // Edit the buffer as client B and see that edit as client A.
        editor_b.update(&mut cx_b, |editor, cx| {
            editor.handle_input(&Input("ok, ".into()), cx)
        });
        buffer_a
            .condition(&cx_a, |buffer, _| buffer.text() == "ok, b-contents")
            .await;

        // Remove the selection set as client B, see those selections disappear as client A.
        cx_b.update(move |_| drop(editor_b));
        buffer_a
            .condition(&cx_a, |buffer, _| buffer.selection_sets().count() == 0)
            .await;

        // Close the buffer as client A, see that the buffer is closed.
        cx_a.update(move |_| drop(buffer_a));
        worktree_a
            .condition(&cx_a, |tree, cx| !tree.has_open_buffer("b.txt", cx))
            .await;

        // Dropping the worktree removes client B from client A's peers.
        cx_b.update(move |_| drop(worktree_b));
        worktree_a
            .condition(&cx_a, |tree, _| tree.peers().is_empty())
            .await;
    }

    #[gpui::test]
    async fn test_unshare_worktree(mut cx_a: TestAppContext, mut cx_b: TestAppContext) {
        cx_b.update(zed::people_panel::init);
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, user_store_b) = server.create_client(&mut cx_b, "user_b").await;
        let mut workspace_b_params = cx_b.update(WorkspaceParams::test);
        workspace_b_params.client = client_b;
        workspace_b_params.user_store = user_store_b;

        cx_a.foreground().forbid_parking();

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/a",
            json!({
                ".zed.toml": r#"collaborators = ["user_b"]"#,
                "a.txt": "a-contents",
                "b.txt": "b-contents",
            }),
        )
        .await;
        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/a".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;

        let remote_worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        let (window_b, workspace_b) = cx_b.add_window(|cx| Workspace::new(&workspace_b_params, cx));
        cx_b.update(|cx| {
            cx.dispatch_action(
                window_b,
                vec![workspace_b.id()],
                &JoinWorktree(remote_worktree_id),
            );
        });
        workspace_b
            .condition(&cx_b, |workspace, cx| workspace.worktrees(cx).len() == 1)
            .await;

        let local_worktree_id_b = workspace_b.read_with(&cx_b, |workspace, cx| {
            let active_pane = workspace.active_pane().read(cx);
            assert!(active_pane.active_item().is_none());
            workspace.worktrees(cx).first().unwrap().id()
        });
        workspace_b
            .update(&mut cx_b, |workspace, cx| {
                workspace.open_entry(
                    ProjectPath {
                        worktree_id: local_worktree_id_b,
                        path: Path::new("a.txt").into(),
                    },
                    cx,
                )
            })
            .unwrap()
            .await;
        workspace_b.read_with(&cx_b, |workspace, cx| {
            let active_pane = workspace.active_pane().read(cx);
            assert!(active_pane.active_item().is_some());
        });

        worktree_a.update(&mut cx_a, |tree, cx| {
            tree.as_local_mut().unwrap().unshare(cx);
        });
        workspace_b
            .condition(&cx_b, |workspace, cx| workspace.worktrees(cx).len() == 0)
            .await;
        workspace_b.read_with(&cx_b, |workspace, cx| {
            let active_pane = workspace.active_pane().read(cx);
            assert!(active_pane.active_item().is_none());
        });
    }

    #[gpui::test]
    async fn test_propagate_saves_and_fs_changes_in_shared_worktree(
        mut cx_a: TestAppContext,
        mut cx_b: TestAppContext,
        mut cx_c: TestAppContext,
    ) {
        cx_a.foreground().forbid_parking();
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 3 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_b, "user_b").await;
        let (client_c, _) = server.create_client(&mut cx_c, "user_c").await;

        let fs = Arc::new(FakeFs::new());

        // Share a worktree as client A.
        fs.insert_tree(
            "/a",
            json!({
                ".zed.toml": r#"collaborators = ["user_b", "user_c"]"#,
                "file1": "",
                "file2": ""
            }),
        )
        .await;

        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/a".as_ref(),
            fs.clone(),
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Join that worktree as clients B and C.
        let worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();
        let worktree_c = Worktree::open_remote(
            client_c.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_c.to_async(),
        )
        .await
        .unwrap();

        // Open and edit a buffer as both guests B and C.
        let buffer_b = worktree_b
            .update(&mut cx_b, |tree, cx| tree.open_buffer("file1", cx))
            .await
            .unwrap();
        let buffer_c = worktree_c
            .update(&mut cx_c, |tree, cx| tree.open_buffer("file1", cx))
            .await
            .unwrap();
        buffer_b.update(&mut cx_b, |buf, cx| buf.edit([0..0], "i-am-b, ", cx));
        buffer_c.update(&mut cx_c, |buf, cx| buf.edit([0..0], "i-am-c, ", cx));

        // Open and edit that buffer as the host.
        let buffer_a = worktree_a
            .update(&mut cx_a, |tree, cx| tree.open_buffer("file1", cx))
            .await
            .unwrap();

        buffer_a
            .condition(&mut cx_a, |buf, _| buf.text() == "i-am-c, i-am-b, ")
            .await;
        buffer_a.update(&mut cx_a, |buf, cx| {
            buf.edit([buf.len()..buf.len()], "i-am-a", cx)
        });

        // Wait for edits to propagate
        buffer_a
            .condition(&mut cx_a, |buf, _| buf.text() == "i-am-c, i-am-b, i-am-a")
            .await;
        buffer_b
            .condition(&mut cx_b, |buf, _| buf.text() == "i-am-c, i-am-b, i-am-a")
            .await;
        buffer_c
            .condition(&mut cx_c, |buf, _| buf.text() == "i-am-c, i-am-b, i-am-a")
            .await;

        // Edit the buffer as the host and concurrently save as guest B.
        let save_b = buffer_b.update(&mut cx_b, |buf, cx| buf.save(cx).unwrap());
        buffer_a.update(&mut cx_a, |buf, cx| buf.edit([0..0], "hi-a, ", cx));
        save_b.await.unwrap();
        assert_eq!(
            fs.load("/a/file1".as_ref()).await.unwrap(),
            "hi-a, i-am-c, i-am-b, i-am-a"
        );
        buffer_a.read_with(&cx_a, |buf, _| assert!(!buf.is_dirty()));
        buffer_b.read_with(&cx_b, |buf, _| assert!(!buf.is_dirty()));
        buffer_c.condition(&cx_c, |buf, _| !buf.is_dirty()).await;

        // Make changes on host's file system, see those changes on the guests.
        fs.rename("/a/file2".as_ref(), "/a/file3".as_ref())
            .await
            .unwrap();
        fs.insert_file(Path::new("/a/file4"), "4".into())
            .await
            .unwrap();

        worktree_b
            .condition(&cx_b, |tree, _| tree.file_count() == 4)
            .await;
        worktree_c
            .condition(&cx_c, |tree, _| tree.file_count() == 4)
            .await;
        worktree_b.read_with(&cx_b, |tree, _| {
            assert_eq!(
                tree.paths()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>(),
                &[".zed.toml", "file1", "file3", "file4"]
            )
        });
        worktree_c.read_with(&cx_c, |tree, _| {
            assert_eq!(
                tree.paths()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>(),
                &[".zed.toml", "file1", "file3", "file4"]
            )
        });
    }

    #[gpui::test]
    async fn test_buffer_conflict_after_save(mut cx_a: TestAppContext, mut cx_b: TestAppContext) {
        cx_a.foreground().forbid_parking();
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_b, "user_b").await;

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/dir",
            json!({
                ".zed.toml": r#"collaborators = ["user_b", "user_c"]"#,
                "a.txt": "a-contents",
            }),
        )
        .await;

        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/dir".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Join that worktree as client B, and see that a guest has joined as client A.
        let worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();

        let buffer_b = worktree_b
            .update(&mut cx_b, |worktree, cx| worktree.open_buffer("a.txt", cx))
            .await
            .unwrap();
        let mtime = buffer_b.read_with(&cx_b, |buf, _| buf.file().unwrap().mtime());

        buffer_b.update(&mut cx_b, |buf, cx| buf.edit([0..0], "world ", cx));
        buffer_b.read_with(&cx_b, |buf, _| {
            assert!(buf.is_dirty());
            assert!(!buf.has_conflict());
        });

        buffer_b
            .update(&mut cx_b, |buf, cx| buf.save(cx))
            .unwrap()
            .await
            .unwrap();
        worktree_b
            .condition(&cx_b, |_, cx| {
                buffer_b.read(cx).file().unwrap().mtime() != mtime
            })
            .await;
        buffer_b.read_with(&cx_b, |buf, _| {
            assert!(!buf.is_dirty());
            assert!(!buf.has_conflict());
        });

        buffer_b.update(&mut cx_b, |buf, cx| buf.edit([0..0], "hello ", cx));
        buffer_b.read_with(&cx_b, |buf, _| {
            assert!(buf.is_dirty());
            assert!(!buf.has_conflict());
        });
    }

    #[gpui::test]
    async fn test_editing_while_guest_opens_buffer(
        mut cx_a: TestAppContext,
        mut cx_b: TestAppContext,
    ) {
        cx_a.foreground().forbid_parking();
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_b, "user_b").await;

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/dir",
            json!({
                ".zed.toml": r#"collaborators = ["user_b"]"#,
                "a.txt": "a-contents",
            }),
        )
        .await;
        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/dir".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Join that worktree as client B, and see that a guest has joined as client A.
        let worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();

        let buffer_a = worktree_a
            .update(&mut cx_a, |tree, cx| tree.open_buffer("a.txt", cx))
            .await
            .unwrap();
        let buffer_b = cx_b
            .background()
            .spawn(worktree_b.update(&mut cx_b, |worktree, cx| worktree.open_buffer("a.txt", cx)));

        task::yield_now().await;
        buffer_a.update(&mut cx_a, |buf, cx| buf.edit([0..0], "z", cx));

        let text = buffer_a.read_with(&cx_a, |buf, _| buf.text());
        let buffer_b = buffer_b.await.unwrap();
        buffer_b.condition(&cx_b, |buf, _| buf.text() == text).await;
    }

    #[gpui::test]
    async fn test_leaving_worktree_while_opening_buffer(
        mut cx_a: TestAppContext,
        mut cx_b: TestAppContext,
    ) {
        cx_a.foreground().forbid_parking();
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_b, "user_b").await;

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/dir",
            json!({
                ".zed.toml": r#"collaborators = ["user_b"]"#,
                "a.txt": "a-contents",
            }),
        )
        .await;
        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/dir".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Join that worktree as client B, and see that a guest has joined as client A.
        let worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .condition(&cx_a, |tree, _| tree.peers().len() == 1)
            .await;

        let buffer_b = cx_b
            .background()
            .spawn(worktree_b.update(&mut cx_b, |worktree, cx| worktree.open_buffer("a.txt", cx)));
        cx_b.update(|_| drop(worktree_b));
        drop(buffer_b);
        worktree_a
            .condition(&cx_a, |tree, _| tree.peers().len() == 0)
            .await;
    }

    #[gpui::test]
    async fn test_peer_disconnection(mut cx_a: TestAppContext, cx_b: TestAppContext) {
        cx_a.foreground().forbid_parking();
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_a, "user_b").await;

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/a",
            json!({
                ".zed.toml": r#"collaborators = ["user_b"]"#,
                "a.txt": "a-contents",
                "b.txt": "b-contents",
            }),
        )
        .await;
        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/a".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Join that worktree as client B, and see that a guest has joined as client A.
        let _worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .condition(&cx_a, |tree, _| tree.peers().len() == 1)
            .await;

        // Drop client B's connection and ensure client A observes client B leaving the worktree.
        client_b.disconnect(&cx_b.to_async()).await.unwrap();
        worktree_a
            .condition(&cx_a, |tree, _| tree.peers().len() == 0)
            .await;
    }

    #[gpui::test]
    async fn test_collaborating_with_diagnostics(
        mut cx_a: TestAppContext,
        mut cx_b: TestAppContext,
    ) {
        cx_a.foreground().forbid_parking();
        let (language_server_config, mut fake_language_server) =
            LanguageServerConfig::fake(cx_a.background()).await;
        let mut lang_registry = LanguageRegistry::new();
        lang_registry.add(Arc::new(Language::new(
            LanguageConfig {
                name: "Rust".to_string(),
                path_suffixes: vec!["rs".to_string()],
                language_server: Some(language_server_config),
                ..Default::default()
            },
            tree_sitter_rust::language(),
        )));

        let lang_registry = Arc::new(lang_registry);

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, _) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, _) = server.create_client(&mut cx_a, "user_b").await;

        // Share a local worktree as client A
        let fs = Arc::new(FakeFs::new());
        fs.insert_tree(
            "/a",
            json!({
                ".zed.toml": r#"collaborators = ["user_b"]"#,
                "a.rs": "let one = two",
                "other.rs": "",
            }),
        )
        .await;
        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/a".as_ref(),
            fs,
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();
        worktree_a
            .read_with(&cx_a, |tree, _| tree.as_local().unwrap().scan_complete())
            .await;
        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        // Cause language server to start.
        let _ = cx_a
            .background()
            .spawn(worktree_a.update(&mut cx_a, |worktree, cx| {
                worktree.open_buffer("other.rs", cx)
            }))
            .await
            .unwrap();

        // Simulate a language server reporting errors for a file.
        fake_language_server
            .notify::<lsp::notification::PublishDiagnostics>(lsp::PublishDiagnosticsParams {
                uri: lsp::Url::from_file_path("/a/a.rs").unwrap(),
                version: None,
                diagnostics: vec![
                    lsp::Diagnostic {
                        severity: Some(lsp::DiagnosticSeverity::ERROR),
                        range: lsp::Range::new(lsp::Position::new(0, 4), lsp::Position::new(0, 7)),
                        message: "message 1".to_string(),
                        ..Default::default()
                    },
                    lsp::Diagnostic {
                        severity: Some(lsp::DiagnosticSeverity::WARNING),
                        range: lsp::Range::new(
                            lsp::Position::new(0, 10),
                            lsp::Position::new(0, 13),
                        ),
                        message: "message 2".to_string(),
                        ..Default::default()
                    },
                ],
            })
            .await;

        // Join the worktree as client B.
        let worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();

        // Open the file with the errors.
        let buffer_b = cx_b
            .background()
            .spawn(worktree_b.update(&mut cx_b, |worktree, cx| worktree.open_buffer("a.rs", cx)))
            .await
            .unwrap();

        buffer_b.read_with(&cx_b, |buffer, _| {
            assert_eq!(
                buffer
                    .diagnostics_in_range(0..buffer.len())
                    .collect::<Vec<_>>(),
                &[
                    (
                        Point::new(0, 4)..Point::new(0, 7),
                        &Diagnostic {
                            group_id: 0,
                            message: "message 1".to_string(),
                            severity: lsp::DiagnosticSeverity::ERROR,
                            is_primary: true
                        }
                    ),
                    (
                        Point::new(0, 10)..Point::new(0, 13),
                        &Diagnostic {
                            group_id: 1,
                            severity: lsp::DiagnosticSeverity::WARNING,
                            message: "message 2".to_string(),
                            is_primary: true
                        }
                    )
                ]
            );
        });
    }

    #[gpui::test]
    async fn test_basic_chat(mut cx_a: TestAppContext, mut cx_b: TestAppContext) {
        cx_a.foreground().forbid_parking();

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, user_store_a) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, user_store_b) = server.create_client(&mut cx_b, "user_b").await;

        // Create an org that includes these 2 users.
        let db = &server.app_state.db;
        let org_id = db.create_org("Test Org", "test-org").await.unwrap();
        db.add_org_member(org_id, current_user_id(&user_store_a, &cx_a), false)
            .await
            .unwrap();
        db.add_org_member(org_id, current_user_id(&user_store_b, &cx_b), false)
            .await
            .unwrap();

        // Create a channel that includes all the users.
        let channel_id = db.create_org_channel(org_id, "test-channel").await.unwrap();
        db.add_channel_member(channel_id, current_user_id(&user_store_a, &cx_a), false)
            .await
            .unwrap();
        db.add_channel_member(channel_id, current_user_id(&user_store_b, &cx_b), false)
            .await
            .unwrap();
        db.create_channel_message(
            channel_id,
            current_user_id(&user_store_b, &cx_b),
            "hello A, it's B.",
            OffsetDateTime::now_utc(),
            1,
        )
        .await
        .unwrap();

        let channels_a = cx_a.add_model(|cx| ChannelList::new(user_store_a, client_a, cx));
        channels_a
            .condition(&mut cx_a, |list, _| list.available_channels().is_some())
            .await;
        channels_a.read_with(&cx_a, |list, _| {
            assert_eq!(
                list.available_channels().unwrap(),
                &[ChannelDetails {
                    id: channel_id.to_proto(),
                    name: "test-channel".to_string()
                }]
            )
        });
        let channel_a = channels_a.update(&mut cx_a, |this, cx| {
            this.get_channel(channel_id.to_proto(), cx).unwrap()
        });
        channel_a.read_with(&cx_a, |channel, _| assert!(channel.messages().is_empty()));
        channel_a
            .condition(&cx_a, |channel, _| {
                channel_messages(channel)
                    == [("user_b".to_string(), "hello A, it's B.".to_string(), false)]
            })
            .await;

        let channels_b = cx_b.add_model(|cx| ChannelList::new(user_store_b, client_b, cx));
        channels_b
            .condition(&mut cx_b, |list, _| list.available_channels().is_some())
            .await;
        channels_b.read_with(&cx_b, |list, _| {
            assert_eq!(
                list.available_channels().unwrap(),
                &[ChannelDetails {
                    id: channel_id.to_proto(),
                    name: "test-channel".to_string()
                }]
            )
        });

        let channel_b = channels_b.update(&mut cx_b, |this, cx| {
            this.get_channel(channel_id.to_proto(), cx).unwrap()
        });
        channel_b.read_with(&cx_b, |channel, _| assert!(channel.messages().is_empty()));
        channel_b
            .condition(&cx_b, |channel, _| {
                channel_messages(channel)
                    == [("user_b".to_string(), "hello A, it's B.".to_string(), false)]
            })
            .await;

        channel_a
            .update(&mut cx_a, |channel, cx| {
                channel
                    .send_message("oh, hi B.".to_string(), cx)
                    .unwrap()
                    .detach();
                let task = channel.send_message("sup".to_string(), cx).unwrap();
                assert_eq!(
                    channel_messages(channel),
                    &[
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_a".to_string(), "oh, hi B.".to_string(), true),
                        ("user_a".to_string(), "sup".to_string(), true)
                    ]
                );
                task
            })
            .await
            .unwrap();

        channel_b
            .condition(&cx_b, |channel, _| {
                channel_messages(channel)
                    == [
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_a".to_string(), "oh, hi B.".to_string(), false),
                        ("user_a".to_string(), "sup".to_string(), false),
                    ]
            })
            .await;

        assert_eq!(
            server
                .state()
                .await
                .channel(channel_id)
                .unwrap()
                .connection_ids
                .len(),
            2
        );
        cx_b.update(|_| drop(channel_b));
        server
            .condition(|state| state.channel(channel_id).unwrap().connection_ids.len() == 1)
            .await;

        cx_a.update(|_| drop(channel_a));
        server
            .condition(|state| state.channel(channel_id).is_none())
            .await;
    }

    #[gpui::test]
    async fn test_chat_message_validation(mut cx_a: TestAppContext) {
        cx_a.foreground().forbid_parking();

        let mut server = TestServer::start().await;
        let (client_a, user_store_a) = server.create_client(&mut cx_a, "user_a").await;

        let db = &server.app_state.db;
        let org_id = db.create_org("Test Org", "test-org").await.unwrap();
        let channel_id = db.create_org_channel(org_id, "test-channel").await.unwrap();
        db.add_org_member(org_id, current_user_id(&user_store_a, &cx_a), false)
            .await
            .unwrap();
        db.add_channel_member(channel_id, current_user_id(&user_store_a, &cx_a), false)
            .await
            .unwrap();

        let channels_a = cx_a.add_model(|cx| ChannelList::new(user_store_a, client_a, cx));
        channels_a
            .condition(&mut cx_a, |list, _| list.available_channels().is_some())
            .await;
        let channel_a = channels_a.update(&mut cx_a, |this, cx| {
            this.get_channel(channel_id.to_proto(), cx).unwrap()
        });

        // Messages aren't allowed to be too long.
        channel_a
            .update(&mut cx_a, |channel, cx| {
                let long_body = "this is long.\n".repeat(1024);
                channel.send_message(long_body, cx).unwrap()
            })
            .await
            .unwrap_err();

        // Messages aren't allowed to be blank.
        channel_a.update(&mut cx_a, |channel, cx| {
            channel.send_message(String::new(), cx).unwrap_err()
        });

        // Leading and trailing whitespace are trimmed.
        channel_a
            .update(&mut cx_a, |channel, cx| {
                channel
                    .send_message("\n surrounded by whitespace  \n".to_string(), cx)
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(
            db.get_channel_messages(channel_id, 10, None)
                .await
                .unwrap()
                .iter()
                .map(|m| &m.body)
                .collect::<Vec<_>>(),
            &["surrounded by whitespace"]
        );
    }

    #[gpui::test]
    async fn test_chat_reconnection(mut cx_a: TestAppContext, mut cx_b: TestAppContext) {
        cx_a.foreground().forbid_parking();

        // Connect to a server as 2 clients.
        let mut server = TestServer::start().await;
        let (client_a, user_store_a) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, user_store_b) = server.create_client(&mut cx_b, "user_b").await;
        let mut status_b = client_b.status();

        // Create an org that includes these 2 users.
        let db = &server.app_state.db;
        let org_id = db.create_org("Test Org", "test-org").await.unwrap();
        db.add_org_member(org_id, current_user_id(&user_store_a, &cx_a), false)
            .await
            .unwrap();
        db.add_org_member(org_id, current_user_id(&user_store_b, &cx_b), false)
            .await
            .unwrap();

        // Create a channel that includes all the users.
        let channel_id = db.create_org_channel(org_id, "test-channel").await.unwrap();
        db.add_channel_member(channel_id, current_user_id(&user_store_a, &cx_a), false)
            .await
            .unwrap();
        db.add_channel_member(channel_id, current_user_id(&user_store_b, &cx_b), false)
            .await
            .unwrap();
        db.create_channel_message(
            channel_id,
            current_user_id(&user_store_b, &cx_b),
            "hello A, it's B.",
            OffsetDateTime::now_utc(),
            2,
        )
        .await
        .unwrap();

        let channels_a = cx_a.add_model(|cx| ChannelList::new(user_store_a, client_a, cx));
        channels_a
            .condition(&mut cx_a, |list, _| list.available_channels().is_some())
            .await;

        channels_a.read_with(&cx_a, |list, _| {
            assert_eq!(
                list.available_channels().unwrap(),
                &[ChannelDetails {
                    id: channel_id.to_proto(),
                    name: "test-channel".to_string()
                }]
            )
        });
        let channel_a = channels_a.update(&mut cx_a, |this, cx| {
            this.get_channel(channel_id.to_proto(), cx).unwrap()
        });
        channel_a.read_with(&cx_a, |channel, _| assert!(channel.messages().is_empty()));
        channel_a
            .condition(&cx_a, |channel, _| {
                channel_messages(channel)
                    == [("user_b".to_string(), "hello A, it's B.".to_string(), false)]
            })
            .await;

        let channels_b = cx_b.add_model(|cx| ChannelList::new(user_store_b.clone(), client_b, cx));
        channels_b
            .condition(&mut cx_b, |list, _| list.available_channels().is_some())
            .await;
        channels_b.read_with(&cx_b, |list, _| {
            assert_eq!(
                list.available_channels().unwrap(),
                &[ChannelDetails {
                    id: channel_id.to_proto(),
                    name: "test-channel".to_string()
                }]
            )
        });

        let channel_b = channels_b.update(&mut cx_b, |this, cx| {
            this.get_channel(channel_id.to_proto(), cx).unwrap()
        });
        channel_b.read_with(&cx_b, |channel, _| assert!(channel.messages().is_empty()));
        channel_b
            .condition(&cx_b, |channel, _| {
                channel_messages(channel)
                    == [("user_b".to_string(), "hello A, it's B.".to_string(), false)]
            })
            .await;

        // Disconnect client B, ensuring we can still access its cached channel data.
        server.forbid_connections();
        server.disconnect_client(current_user_id(&user_store_b, &cx_b));
        while !matches!(
            status_b.recv().await,
            Some(client::Status::ReconnectionError { .. })
        ) {}

        channels_b.read_with(&cx_b, |channels, _| {
            assert_eq!(
                channels.available_channels().unwrap(),
                [ChannelDetails {
                    id: channel_id.to_proto(),
                    name: "test-channel".to_string()
                }]
            )
        });
        channel_b.read_with(&cx_b, |channel, _| {
            assert_eq!(
                channel_messages(channel),
                [("user_b".to_string(), "hello A, it's B.".to_string(), false)]
            )
        });

        // Send a message from client B while it is disconnected.
        channel_b
            .update(&mut cx_b, |channel, cx| {
                let task = channel
                    .send_message("can you see this?".to_string(), cx)
                    .unwrap();
                assert_eq!(
                    channel_messages(channel),
                    &[
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_b".to_string(), "can you see this?".to_string(), true)
                    ]
                );
                task
            })
            .await
            .unwrap_err();

        // Send a message from client A while B is disconnected.
        channel_a
            .update(&mut cx_a, |channel, cx| {
                channel
                    .send_message("oh, hi B.".to_string(), cx)
                    .unwrap()
                    .detach();
                let task = channel.send_message("sup".to_string(), cx).unwrap();
                assert_eq!(
                    channel_messages(channel),
                    &[
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_a".to_string(), "oh, hi B.".to_string(), true),
                        ("user_a".to_string(), "sup".to_string(), true)
                    ]
                );
                task
            })
            .await
            .unwrap();

        // Give client B a chance to reconnect.
        server.allow_connections();
        cx_b.foreground().advance_clock(Duration::from_secs(10));

        // Verify that B sees the new messages upon reconnection, as well as the message client B
        // sent while offline.
        channel_b
            .condition(&cx_b, |channel, _| {
                channel_messages(channel)
                    == [
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_a".to_string(), "oh, hi B.".to_string(), false),
                        ("user_a".to_string(), "sup".to_string(), false),
                        ("user_b".to_string(), "can you see this?".to_string(), false),
                    ]
            })
            .await;

        // Ensure client A and B can communicate normally after reconnection.
        channel_a
            .update(&mut cx_a, |channel, cx| {
                channel.send_message("you online?".to_string(), cx).unwrap()
            })
            .await
            .unwrap();
        channel_b
            .condition(&cx_b, |channel, _| {
                channel_messages(channel)
                    == [
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_a".to_string(), "oh, hi B.".to_string(), false),
                        ("user_a".to_string(), "sup".to_string(), false),
                        ("user_b".to_string(), "can you see this?".to_string(), false),
                        ("user_a".to_string(), "you online?".to_string(), false),
                    ]
            })
            .await;

        channel_b
            .update(&mut cx_b, |channel, cx| {
                channel.send_message("yep".to_string(), cx).unwrap()
            })
            .await
            .unwrap();
        channel_a
            .condition(&cx_a, |channel, _| {
                channel_messages(channel)
                    == [
                        ("user_b".to_string(), "hello A, it's B.".to_string(), false),
                        ("user_a".to_string(), "oh, hi B.".to_string(), false),
                        ("user_a".to_string(), "sup".to_string(), false),
                        ("user_b".to_string(), "can you see this?".to_string(), false),
                        ("user_a".to_string(), "you online?".to_string(), false),
                        ("user_b".to_string(), "yep".to_string(), false),
                    ]
            })
            .await;
    }

    #[gpui::test]
    async fn test_collaborators(
        mut cx_a: TestAppContext,
        mut cx_b: TestAppContext,
        mut cx_c: TestAppContext,
    ) {
        cx_a.foreground().forbid_parking();
        let lang_registry = Arc::new(LanguageRegistry::new());

        // Connect to a server as 3 clients.
        let mut server = TestServer::start().await;
        let (client_a, user_store_a) = server.create_client(&mut cx_a, "user_a").await;
        let (client_b, user_store_b) = server.create_client(&mut cx_b, "user_b").await;
        let (_client_c, user_store_c) = server.create_client(&mut cx_c, "user_c").await;

        let fs = Arc::new(FakeFs::new());

        // Share a worktree as client A.
        fs.insert_tree(
            "/a",
            json!({
                ".zed.toml": r#"collaborators = ["user_b", "user_c"]"#,
            }),
        )
        .await;

        let worktree_a = Worktree::open_local(
            client_a.clone(),
            "/a".as_ref(),
            fs.clone(),
            lang_registry.clone(),
            &mut cx_a.to_async(),
        )
        .await
        .unwrap();

        user_store_a
            .condition(&cx_a, |user_store, _| {
                collaborators(user_store) == vec![("user_a", vec![("a", vec![])])]
            })
            .await;
        user_store_b
            .condition(&cx_b, |user_store, _| {
                collaborators(user_store) == vec![("user_a", vec![("a", vec![])])]
            })
            .await;
        user_store_c
            .condition(&cx_c, |user_store, _| {
                collaborators(user_store) == vec![("user_a", vec![("a", vec![])])]
            })
            .await;

        let worktree_id = worktree_a
            .update(&mut cx_a, |tree, cx| tree.as_local_mut().unwrap().share(cx))
            .await
            .unwrap();

        let _worktree_b = Worktree::open_remote(
            client_b.clone(),
            worktree_id,
            lang_registry.clone(),
            &mut cx_b.to_async(),
        )
        .await
        .unwrap();

        user_store_a
            .condition(&cx_a, |user_store, _| {
                collaborators(user_store) == vec![("user_a", vec![("a", vec!["user_b"])])]
            })
            .await;
        user_store_b
            .condition(&cx_b, |user_store, _| {
                collaborators(user_store) == vec![("user_a", vec![("a", vec!["user_b"])])]
            })
            .await;
        user_store_c
            .condition(&cx_c, |user_store, _| {
                collaborators(user_store) == vec![("user_a", vec![("a", vec!["user_b"])])]
            })
            .await;

        cx_a.update(move |_| drop(worktree_a));
        user_store_a
            .condition(&cx_a, |user_store, _| collaborators(user_store) == vec![])
            .await;
        user_store_b
            .condition(&cx_b, |user_store, _| collaborators(user_store) == vec![])
            .await;
        user_store_c
            .condition(&cx_c, |user_store, _| collaborators(user_store) == vec![])
            .await;

        fn collaborators(user_store: &UserStore) -> Vec<(&str, Vec<(&str, Vec<&str>)>)> {
            user_store
                .collaborators()
                .iter()
                .map(|collaborator| {
                    let worktrees = collaborator
                        .worktrees
                        .iter()
                        .map(|w| {
                            (
                                w.root_name.as_str(),
                                w.guests.iter().map(|p| p.github_login.as_str()).collect(),
                            )
                        })
                        .collect();
                    (collaborator.user.github_login.as_str(), worktrees)
                })
                .collect()
        }
    }

    struct TestServer {
        peer: Arc<Peer>,
        app_state: Arc<AppState>,
        server: Arc<Server>,
        notifications: mpsc::Receiver<()>,
        connection_killers: Arc<Mutex<HashMap<UserId, watch::Sender<Option<()>>>>>,
        forbid_connections: Arc<AtomicBool>,
        _test_db: TestDb,
    }

    impl TestServer {
        async fn start() -> Self {
            let test_db = TestDb::new();
            let app_state = Self::build_app_state(&test_db).await;
            let peer = Peer::new();
            let notifications = mpsc::channel(128);
            let server = Server::new(app_state.clone(), peer.clone(), Some(notifications.0));
            Self {
                peer,
                app_state,
                server,
                notifications: notifications.1,
                connection_killers: Default::default(),
                forbid_connections: Default::default(),
                _test_db: test_db,
            }
        }

        async fn create_client(
            &mut self,
            cx: &mut TestAppContext,
            name: &str,
        ) -> (Arc<Client>, ModelHandle<UserStore>) {
            let user_id = self.app_state.db.create_user(name, false).await.unwrap();
            let client_name = name.to_string();
            let mut client = Client::new();
            let server = self.server.clone();
            let connection_killers = self.connection_killers.clone();
            let forbid_connections = self.forbid_connections.clone();
            Arc::get_mut(&mut client)
                .unwrap()
                .override_authenticate(move |cx| {
                    cx.spawn(|_| async move {
                        let access_token = "the-token".to_string();
                        Ok(Credentials {
                            user_id: user_id.0 as u64,
                            access_token,
                        })
                    })
                })
                .override_establish_connection(move |credentials, cx| {
                    assert_eq!(credentials.user_id, user_id.0 as u64);
                    assert_eq!(credentials.access_token, "the-token");

                    let server = server.clone();
                    let connection_killers = connection_killers.clone();
                    let forbid_connections = forbid_connections.clone();
                    let client_name = client_name.clone();
                    cx.spawn(move |cx| async move {
                        if forbid_connections.load(SeqCst) {
                            Err(EstablishConnectionError::other(anyhow!(
                                "server is forbidding connections"
                            )))
                        } else {
                            let (client_conn, server_conn, kill_conn) = Connection::in_memory();
                            connection_killers.lock().insert(user_id, kill_conn);
                            cx.background()
                                .spawn(server.handle_connection(server_conn, client_name, user_id))
                                .detach();
                            Ok(client_conn)
                        }
                    })
                });

            let http = FakeHttpClient::new(|_| async move { Ok(surf::http::Response::new(404)) });
            client
                .authenticate_and_connect(&cx.to_async())
                .await
                .unwrap();

            let user_store = cx.add_model(|cx| UserStore::new(client.clone(), http, cx));
            let mut authed_user =
                user_store.read_with(cx, |user_store, _| user_store.watch_current_user());
            while authed_user.recv().await.unwrap().is_none() {}

            (client, user_store)
        }

        fn disconnect_client(&self, user_id: UserId) {
            if let Some(mut kill_conn) = self.connection_killers.lock().remove(&user_id) {
                let _ = kill_conn.try_send(Some(()));
            }
        }

        fn forbid_connections(&self) {
            self.forbid_connections.store(true, SeqCst);
        }

        fn allow_connections(&self) {
            self.forbid_connections.store(false, SeqCst);
        }

        async fn build_app_state(test_db: &TestDb) -> Arc<AppState> {
            let mut config = Config::default();
            config.session_secret = "a".repeat(32);
            config.database_url = test_db.url.clone();
            let github_client = github::AppClient::test();
            Arc::new(AppState {
                db: test_db.db().clone(),
                handlebars: Default::default(),
                auth_client: auth::build_client("", ""),
                repo_client: github::RepoClient::test(&github_client),
                github_client,
                config,
            })
        }

        async fn state<'a>(&'a self) -> RwLockReadGuard<'a, Store> {
            self.server.store.read()
        }

        async fn condition<F>(&mut self, mut predicate: F)
        where
            F: FnMut(&Store) -> bool,
        {
            async_std::future::timeout(Duration::from_millis(500), async {
                while !(predicate)(&*self.server.store.read()) {
                    self.notifications.recv().await;
                }
            })
            .await
            .expect("condition timed out");
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            task::block_on(self.peer.reset());
        }
    }

    fn current_user_id(user_store: &ModelHandle<UserStore>, cx: &TestAppContext) -> UserId {
        UserId::from_proto(
            user_store.read_with(cx, |user_store, _| user_store.current_user().unwrap().id),
        )
    }

    fn channel_messages(channel: &Channel) -> Vec<(String, String, bool)> {
        channel
            .messages()
            .cursor::<()>()
            .map(|m| {
                (
                    m.sender.github_login.clone(),
                    m.body.clone(),
                    m.is_pending(),
                )
            })
            .collect()
    }

    struct EmptyView;

    impl gpui::Entity for EmptyView {
        type Event = ();
    }

    impl gpui::View for EmptyView {
        fn ui_name() -> &'static str {
            "empty view"
        }

        fn render(&mut self, _: &mut gpui::RenderContext<Self>) -> gpui::ElementBox {
            gpui::Element::boxed(gpui::elements::Empty)
        }
    }
}
