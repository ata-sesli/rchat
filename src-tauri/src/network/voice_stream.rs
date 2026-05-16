use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    future::{ready, Ready},
    io,
    task::{Context, Poll},
};

use futures::channel::{mpsc, oneshot};
use libp2p::{
    core::{InboundUpgrade, OutboundUpgrade, UpgradeInfo},
    swarm::{
        self, ConnectionDenied, ConnectionHandler, ConnectionId, FromSwarm, NetworkBehaviour,
        Stream, StreamProtocol, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
    },
    Multiaddr, PeerId,
};

#[derive(Debug)]
pub enum OpenStreamError {
    NoSuchConnection,
    UnsupportedProtocol(StreamProtocol),
    Io(io::Error),
}

impl std::fmt::Display for OpenStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchConnection => write!(f, "connection is not available"),
            Self::UnsupportedProtocol(protocol) => {
                write!(f, "remote peer does not support {protocol}")
            }
            Self::Io(err) => write!(f, "io error: {err}"),
        }
    }
}

impl std::error::Error for OpenStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for OpenStreamError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub type OpenStreamReceiver = oneshot::Receiver<Result<Stream, OpenStreamError>>;
pub type IncomingStreams = mpsc::UnboundedReceiver<(PeerId, Stream)>;

pub struct Behaviour {
    protocol: StreamProtocol,
    incoming_sender: mpsc::UnboundedSender<(PeerId, Stream)>,
    incoming_receiver: Option<IncomingStreams>,
    established: HashMap<ConnectionId, PeerId>,
    pending_opens: VecDeque<(PeerId, ConnectionId, HandlerIn)>,
}

impl Behaviour {
    pub fn new(protocol: StreamProtocol) -> Self {
        let (incoming_sender, incoming_receiver) = mpsc::unbounded();
        Self {
            protocol,
            incoming_sender,
            incoming_receiver: Some(incoming_receiver),
            established: HashMap::new(),
            pending_opens: VecDeque::new(),
        }
    }

    pub fn take_incoming(&mut self) -> Option<IncomingStreams> {
        self.incoming_receiver.take()
    }

    pub fn open_stream_on_connection(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
    ) -> Result<OpenStreamReceiver, OpenStreamError> {
        if self.established.get(&connection_id) != Some(&peer_id) {
            return Err(OpenStreamError::NoSuchConnection);
        }

        let (sender, receiver) = oneshot::channel();
        self.pending_opens.push_back((
            peer_id,
            connection_id,
            HandlerIn::OpenStream {
                protocol: self.protocol.clone(),
                sender,
            },
        ));
        Ok(receiver)
    }
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = Handler;
    type ToSwarm = ();

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(Handler::new(self.protocol.clone()))
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role_override: libp2p::core::Endpoint,
        _port_use: libp2p::core::transport::PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(Handler::new(self.protocol.clone()))
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(event) => {
                self.established.insert(event.connection_id, event.peer_id);
            }
            FromSwarm::ConnectionClosed(event) => {
                self.established.remove(&event.connection_id);
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        _connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {
            HandlerOut::Inbound(stream) => {
                let _ = self.incoming_sender.unbounded_send((peer_id, stream));
            }
        }
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        if let Some((peer_id, connection_id, event)) = self.pending_opens.pop_front() {
            return Poll::Ready(ToSwarm::NotifyHandler {
                peer_id,
                handler: swarm::NotifyHandler::One(connection_id),
                event,
            });
        }

        Poll::Pending
    }
}

pub struct Handler {
    supported_protocol: StreamProtocol,
    pending_open: Option<PendingOpen>,
    queued_opens: VecDeque<PendingOpen>,
    queued_inbound: VecDeque<Stream>,
}

impl Handler {
    fn new(protocol: StreamProtocol) -> Self {
        Self {
            supported_protocol: protocol,
            pending_open: None,
            queued_opens: VecDeque::new(),
            queued_inbound: VecDeque::new(),
        }
    }
}

struct PendingOpen {
    protocol: StreamProtocol,
    sender: oneshot::Sender<Result<Stream, OpenStreamError>>,
}

#[derive(Debug)]
pub enum HandlerIn {
    OpenStream {
        protocol: StreamProtocol,
        sender: oneshot::Sender<Result<Stream, OpenStreamError>>,
    },
}

#[derive(Debug)]
pub enum HandlerOut {
    Inbound(Stream),
}

impl ConnectionHandler for Handler {
    type FromBehaviour = HandlerIn;
    type ToBehaviour = HandlerOut;
    type InboundProtocol = Upgrade;
    type OutboundProtocol = Upgrade;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = ();

    fn listen_protocol(&self) -> swarm::SubstreamProtocol<Self::InboundProtocol> {
        swarm::SubstreamProtocol::new(
            Upgrade {
                protocols: vec![self.supported_protocol.clone()],
            },
            (),
        )
    }

    fn on_behaviour_event(&mut self, event: Self::FromBehaviour) {
        match event {
            HandlerIn::OpenStream { protocol, sender } => {
                self.queued_opens
                    .push_back(PendingOpen { protocol, sender });
            }
        }
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<swarm::ConnectionHandlerEvent<Self::OutboundProtocol, (), Self::ToBehaviour>> {
        if let Some(stream) = self.queued_inbound.pop_front() {
            return Poll::Ready(swarm::ConnectionHandlerEvent::NotifyBehaviour(
                HandlerOut::Inbound(stream),
            ));
        }

        if self.pending_open.is_some() {
            return Poll::Pending;
        }

        if let Some(open) = self.queued_opens.pop_front() {
            let protocol = open.protocol.clone();
            self.pending_open = Some(open);
            return Poll::Ready(swarm::ConnectionHandlerEvent::OutboundSubstreamRequest {
                protocol: swarm::SubstreamProtocol::new(
                    Upgrade {
                        protocols: vec![protocol],
                    },
                    (),
                ),
            });
        }

        Poll::Pending
    }

    fn on_connection_event(
        &mut self,
        event: swarm::handler::ConnectionEvent<Self::InboundProtocol, Self::OutboundProtocol>,
    ) {
        use swarm::handler::{ConnectionEvent, DialUpgradeError};

        match event {
            ConnectionEvent::FullyNegotiatedInbound(negotiated) => {
                let (stream, _protocol) = negotiated.protocol;
                self.queued_inbound.push_back(stream);
            }
            ConnectionEvent::FullyNegotiatedOutbound(negotiated) => {
                if let Some(open) = self.pending_open.take() {
                    let (stream, _protocol) = negotiated.protocol;
                    let _ = open.sender.send(Ok(stream));
                }
            }
            ConnectionEvent::DialUpgradeError(DialUpgradeError { error, .. }) => {
                if let Some(open) = self.pending_open.take() {
                    let error = match error {
                        swarm::StreamUpgradeError::Timeout => {
                            OpenStreamError::Io(io::ErrorKind::TimedOut.into())
                        }
                        swarm::StreamUpgradeError::NegotiationFailed => {
                            OpenStreamError::UnsupportedProtocol(open.protocol.clone())
                        }
                        swarm::StreamUpgradeError::Io(err) => OpenStreamError::Io(err),
                        swarm::StreamUpgradeError::Apply(never) => match never {},
                    };
                    let _ = open.sender.send(Err(error));
                }
            }
            _ => {}
        }
    }
}

pub struct Upgrade {
    protocols: Vec<StreamProtocol>,
}

impl UpgradeInfo for Upgrade {
    type Info = StreamProtocol;
    type InfoIter = std::vec::IntoIter<StreamProtocol>;

    fn protocol_info(&self) -> Self::InfoIter {
        self.protocols.clone().into_iter()
    }
}

impl InboundUpgrade<Stream> for Upgrade {
    type Output = (Stream, StreamProtocol);
    type Error = Infallible;
    type Future = Ready<Result<Self::Output, Self::Error>>;

    fn upgrade_inbound(self, socket: Stream, info: Self::Info) -> Self::Future {
        ready(Ok((socket, info)))
    }
}

impl OutboundUpgrade<Stream> for Upgrade {
    type Output = (Stream, StreamProtocol);
    type Error = Infallible;
    type Future = Ready<Result<Self::Output, Self::Error>>;

    fn upgrade_outbound(self, socket: Stream, info: Self::Info) -> Self::Future {
        ready(Ok((socket, info)))
    }
}
