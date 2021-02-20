#[allow(dead_code)]

const MAX_REQUEST_PINGS: usize = 10;
const MAX_DIRECT_NODES: usize = 10;

use std::collections::HashMap;

use priority_queue::PriorityQueue;

pub use crate::internet::{CustomNode, InternetID, InternetPacket};

mod types;
pub use types::{NodeID, SessionID, RouteCoord, NodePacket, NodeEncryption, RemoteNode, RemoteNodeError, RemoteSession, SessionType, SessionError, RouteScalar};


#[derive(Debug, Clone)]
/// A condition that should be satisfied before an action is executed
pub enum NodeActionCondition {
	Session(NodeID), // Has a session (Direct or Routed)
	DirectSession(NodeID), // Has direct Internet Connection
	PeerTested(NodeID), // Has node been considered as candidate for self.direct_node
	Timeout(usize), // Condition runs at some time in the future
	RunFirst(Box<NodeAction>),
}
#[derive(Error, Debug)]
pub enum NodeActionConditionError {
    #[error("Node Error")]
	NodeError(#[from] NodeError),
	#[error("RemoteNode Error")]
	RemoteNodeError(#[from] RemoteNodeError),
}
impl NodeActionCondition {
	fn test(self, node: &mut Node) -> Result<Option<Self>, NodeActionConditionError> {
		Ok(match self {
			// Yields if there is a session
			NodeActionCondition::Session(node_id) => node.remote(&node_id)?.session_active().then(||self),
			// Yields if there is a session and it is direct
			NodeActionCondition::DirectSession(node_id) => {
				let remote = node.remote(&node_id)?;
				(remote.session()?.is_direct() && remote.session_active()).then(||self)
			},
			NodeActionCondition::PeerTested(node_id) => {
				(node.remote_mut(&node_id)?.session_mut()?.direct_mut()?.is_viable() == Some(true)).then(||self)
			},
			// Yields if it is at specified time
			NodeActionCondition::Timeout(time) => (node.ticks >= time).then(||self),
			// Yields and runs nested action
			NodeActionCondition::RunFirst(embedded_action) => {
				node.action(*embedded_action);
				None
			},
		})
	}
}
#[derive(Debug, Clone)]
pub enum NodeAction {
	/// Connect
	Connect(NodeID, InternetID),
	/// Send Packet to NodeID
	Packet(NodeID, NodePacket),
	/// Bootstrap off of directly connected node
	Bootstrap(NodeID),
	/// Ping a node
	Ping(NodeID, usize), // Ping node X number of times
	/// Test a remote to check if it is a viable direct connection candidate
	TestDirect(NodeID),
	/// Establish a dynamic routed connection
	Route(NodeID, RouteCoord),
	/// Condition for a condition to be fulfilled before running imbedded Action
	Condition(NodeActionCondition, Box<NodeAction>),
}
impl NodeAction {
	pub fn gen_condition(self, condition: NodeActionCondition) -> NodeAction {
		NodeAction::Condition(condition, Box::new(self))
	}
}

#[derive(Debug)]
pub struct Node {
	pub node_id: NodeID,
	pub net_id: InternetID,

	my_route: Vec<u16>,
	ticks: usize, // Amount of time passed since startup of this node

	pub remotes: HashMap<NodeID, RemoteNode>,
	pub sessions: HashMap<SessionID, NodeID>,
	pub direct_nodes: PriorityQueue<SessionID, RouteScalar>, // Sort Queue SessionID by distance (use Reverse to access shortest index)
	pub actions_queue: Vec<NodeAction>, // Actions will wait here until NodeID session is established
}
impl CustomNode for Node {
	type CustomNodeAction = NodeAction;
	fn net_id(&self) -> InternetID {
		self.net_id
	}
	fn tick(&mut self, incoming: Vec<InternetPacket>) -> Vec<InternetPacket> {
		let mut outgoing: Vec<InternetPacket> = Vec::new();

		// Parse Incoming Packets
		for packet in incoming {
			//let mut noise = builder.local_private_key(self.keypair.)
			if let Err(err) = self.parse_packet(packet, &mut outgoing) {
				println!("Failed to parse packet: {:?}", err);
			}
		}
		
		// Run actions in queue 
		// This is kinda inefficient
		let mut aq = self.actions_queue.clone();
		let generated_actions = aq.drain_filter(|action| {
			match self.parse_action(&action, &mut outgoing) {
				Ok(resolved) => resolved,
				Err(err) => { log::info!("Action {:?} errored: {:?}", action, err); false },
			}
		}).collect::<Vec<_>>();
		self.actions_queue = aq;
		// Check for Yielded NodeAction::Condition and list embedded action in queue
		for action in generated_actions.into_iter() {
			match action {
				NodeAction::Condition(_, action) => self.actions_queue.push(*action),
				_ => {},
			}
		}

		self.ticks += 1;
		
		outgoing
	}
	fn action(&mut self, action: NodeAction) {
		self.actions_queue.push(action);
	}
}
#[derive(Error, Debug)]
pub enum PacketParseError {
	#[error("Node Error")]
	NodeError(#[from] NodeError),
    #[error("There is no known session: {session_id:?}")]
	UnknownSession { session_id: SessionID },
	#[error("InternetPacket from {from:?} was addressed to {intended_dest:?}, not me")]
	InvalidNetworkRecipient { from: InternetID, intended_dest: InternetID },
	#[error("Handshake was addressed to {node_id:?} and not me")]
	InvalidHandshakeRecipient { node_id: NodeID },
	#[error("Acknowledgement from {from:?} was recieved, but I didn't previously send a Handshake Request")]
	UnknownAcknowledgement { from: NodeID },
	#[error("Triggered RemoteNodeError")]
	RemoteNodeError(#[from] RemoteNodeError),
	#[error("Remote Session Error")]
	SessionError(#[from] SessionError),
	#[error("Failed to decode packet data")]
	SerdeDecodeError(#[from] serde_json::Error),
	#[error("There are no known directly connected nodes")]
	NoDirectNodes,
}
#[derive(Error, Debug)]
pub enum ActionError {
    #[error("Node Error")]
	NodeError(#[from] NodeError),
	#[error("RemoteNode Error")]
	RemoteNodeError(#[from] RemoteNodeError),
	#[error("NodeActionCondition Error")]
	NodeActionConditionError(#[from] NodeActionConditionError),
}
#[derive(Error, Debug)]
pub enum NodeError {
    #[error("There is no known remote: {node_id:?}")]
	NoRemoteError { node_id: NodeID },
}

impl Node {
	pub fn new(node_id: NodeID, net_id: InternetID) -> Node {
		//let keypair = Keypair::generate_ed25519();
		//let node_id = key.public().into_peer_id();
		Node {
			node_id,
			//keypair,
			net_id,

			my_route: Default::default(),
			ticks: Default::default(),

			remotes: Default::default(),
			sessions: Default::default(),
			direct_nodes: Default::default(),
			actions_queue: Default::default(),
		}
	}
	pub fn with_action(mut self, action: NodeAction) -> Self {
		self.actions_queue.push(action);
		self
	}
	pub fn remote(&self, node_id: &NodeID) -> Result<&RemoteNode, NodeError> { self.remotes.get(node_id).ok_or(NodeError::NoRemoteError{node_id: *node_id}) }
	pub fn remote_mut(&mut self, node_id: &NodeID) -> Result<&mut RemoteNode, NodeError> { self.remotes.get_mut(node_id).ok_or(NodeError::NoRemoteError{node_id: *node_id}) }

	pub fn parse_action(&mut self, action: &NodeAction, outgoing: &mut Vec<InternetPacket>) -> Result<bool, ActionError> {
		match action.clone() {
			// Connect to remote node
			NodeAction::Connect(remote_node_id, remote_net_id) => {
				// Insert RemoteNode if doesn't exist
				let remote = self.remotes.entry(remote_node_id).or_insert(RemoteNode::new(remote_node_id));
				// Run Handshake if no active session
				if !remote.session_active() {
					let packet = remote.gen_handshake(self.node_id, RemoteSession::direct(remote_net_id));
					let packet = packet.package(self.net_id, remote_net_id);
					outgoing.push(packet);
				}
			},
			NodeAction::TestDirect(remote_node_id) => {
				let remote = self.remote(&remote_node_id)?;
			},
			NodeAction::Packet(remote_node_id, packet) => {
				let remote = self.remote(&remote_node_id)?;
				outgoing.push(remote.gen_packet(self.net_id, packet)?);
			},
			// Bootstrap off of remote node
			NodeAction::Bootstrap(remote_node_id) => {
				let remote = self.remote(&remote_node_id)?;
				let packet = remote.gen_packet(self.net_id, NodePacket::RequestPings(10))?;
				outgoing.push(packet);
			},
			NodeAction::Ping(remote_node_id, num_pings) => {
				let self_net_id = self.net_id;
				let self_ticks = self.ticks;
				let session = self.remote_mut(&remote_node_id)?.session_mut()?;
				let session_id = session.session_id;
				let direct_session = session.direct_mut()?;
				let remote_net_id = direct_session.net_id;
				for i in 0..num_pings {
					let ping_packet = direct_session.gen_ping(self_ticks).encrypt(session_id);
					let packet = ping_packet.package(self_net_id, remote_net_id);
					outgoing.push(packet);
				}
			},
			NodeAction::Route(remote_node_id, remote_route_coord ) => {},
			NodeAction::Condition(condition, action) => {
				return Ok(condition.test(self)?.is_some());
			}
			// _ => { log::error!("Invalid NodeAction / NodeActionCondition pair"); },
		}
		Ok(true)
	}
	pub fn parse_packet(&mut self, received_packet: InternetPacket, outgoing: &mut Vec<InternetPacket>) -> Result<(), PacketParseError> {
		if received_packet.dest_addr == self.net_id {
			use NodeEncryption::*;
			let encrypted = NodeEncryption::unpackage(&received_packet)?;
			match encrypted {
				Handshake { recipient, session_id, signer } => {
					log::info!("Node({:?}) Received Handshake: {:?}", self.node_id, encrypted);
					if recipient == self.node_id {
						// If receive a Handshake Request, acknowledge it
						let remote = self.remotes.entry(signer).or_insert(RemoteNode::new(signer));
						let acknowledge_packet = remote.gen_acknowledgement(recipient, session_id);
						self.sessions.insert(session_id, signer); // Register to SessionID index
						outgoing.push(acknowledge_packet.package(self.net_id, received_packet.src_addr));
					} else {
						return Err( PacketParseError::InvalidHandshakeRecipient { node_id: recipient } )
					}
				},
				Acknowledge { session_id, acknowledger } => {
					log::info!("Node({:?}) Received Acknowledgement: {:?}", self.node_id, encrypted);
					// If receive an Acknowledge request, validate Handshake previously sent out
					let remote = self.remote_mut(&acknowledger)?;
					remote.validate_handshake(session_id, acknowledger)?;
					self.sessions.insert(session_id, acknowledger); // Register to SessionID index
				},
				Session { session_id, packet: node_packet } => {
					let return_node_id = *self.sessions.get(&session_id).ok_or(PacketParseError::UnknownSession { session_id })?;
					log::info!("Node({}) received NodePacket::{:?} from NodeID({}), InternetID({})", self.node_id, node_packet, return_node_id, received_packet.src_addr);
					//let return_remote = self.remote_mut(&return_node_id)?;
					match node_packet {
						NodePacket::Ping(ping_id) => {
							// Return packet
							let session = self.remote_mut(&return_node_id)?.session()?;
							let packet = session.encrypt(NodePacket::PingResponse(ping_id)).package(self.net_id, received_packet.src_addr);
							outgoing.push(packet);
						},
						NodePacket::PingResponse(ping_id) => {
							// Acknowledge ping
							let ticks = self.ticks;
							let session = self.remote_mut(&return_node_id)?.session_mut()?;
							if let SessionType::Direct(direct_session) = &mut session.session_type {
								direct_session.acknowledge_ping(ping_id, ticks)?;
								// Log direct nodes
								let session_id = session.session_id;
								let dist = direct_session.distance();
								self.direct_nodes.push(session_id, dist);
							}
						},
						NodePacket::DirectRequest => {
							self.action(NodeAction::TestDirect(return_node_id));
						}
						NodePacket::RequestPings(requests) => {
							// Loop through first min(N,MAX_REQUEST_PINGS) items of priorityqueue
							let num_requests = usize::min(requests, MAX_REQUEST_PINGS); // Maximum of 10 requests
							for (session_id, _) in self.direct_nodes.iter().take(num_requests) {
								// Try get node
								let node_id = self.sessions.get(session_id).ok_or(PacketParseError::UnknownSession { session_id: *session_id })?;
								// Try get remote
								let remote = self.remote(node_id)?;
								// Generate packet sent to nearby remotes that this node wants to be pinged
								let packet = remote.gen_packet(self.net_id, NodePacket::WantPing(return_node_id, received_packet.dest_addr))?;
								outgoing.push(packet);
							}
							// TODO: Find nodes that might be close to requester and ask them to ping requester
						},
						// Initiate Direct Handshakes with people who want pings
						NodePacket::WantPing(requesting_node_id, requesting_net_id) => {
							let remote_node = self.remotes.entry(requesting_node_id).or_insert(RemoteNode::new(requesting_node_id));
							// Connect to requestied node
							self.action(NodeAction::Connect(requesting_node_id, requesting_net_id));
							let packet_action = NodeAction::Packet(requesting_node_id, NodePacket::AcceptWantPing(return_node_id))
								.gen_condition(NodeActionCondition::Timeout(self.ticks + 300));
							self.action(packet_action);
						},
						NodePacket::RouteRequest(target_coord, max_distance, requester_coord, requester_node_id) => {
							// outgoing.push(value)
						},
						_ => { },
					}
				}
			}
		} else {
			return Err( PacketParseError::InvalidNetworkRecipient { from: received_packet.src_addr, intended_dest: received_packet.dest_addr } )
		}
		Ok(())
	}
}
