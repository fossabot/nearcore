use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use borsh::{BorshDeserialize, BorshSerialize};
use byteorder::WriteBytesExt;
use bytes::LittleEndian;
use cached::{Cached, SizedCache};
use log::{debug, trace};

use near_crypto::{SecretKey, Signature};
use near_primitives::hash::{hash, CryptoHash};
use near_primitives::types::AccountId;

use crate::types::{AnnounceAccount, PeerId, PeerIdOrHash, Ping, Pong};
use crate::utils::CloneNone;

const ROUTE_BACK_CACHE_SIZE: usize = 10000;
const ROUND_ROBIN_MAX_NONCE_DIFFERENCE_ALLOWED: usize = 10;

/// Information that will be ultimately used to create a new edge.
/// It contains nonce proposed for the edge with signature from peer.
#[derive(Clone, BorshSerialize, BorshDeserialize, PartialEq, Eq, Debug, Default)]
pub struct EdgeInfo {
    pub nonce: u64,
    pub signature: Signature,
}

impl EdgeInfo {
    pub fn new(peer0: PeerId, peer1: PeerId, nonce: u64, secret_key: &SecretKey) -> Self {
        let (peer0, peer1) = Edge::key(peer0, peer1);
        let data = Edge::build_hash(&peer0, &peer1, nonce);
        let signature = secret_key.sign(data.as_ref());
        Self { nonce, signature }
    }
}

/// Status of the edge
#[derive(BorshSerialize, BorshDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum EdgeType {
    Added,
    Removed,
}

/// Edge object. Contains information relative to a new edge that is being added or removed
/// from the network. This is the information that is required
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Since edges are not directed `peer0 < peer1` should hold.
    pub peer0: PeerId,
    pub peer1: PeerId,
    /// Nonce to keep tracking of the last update on this edge.
    /// It must be even
    pub nonce: u64,
    /// Signature from parties validating the edge. These are signature of the added edge.
    signature0: Signature,
    signature1: Signature,
    /// Info necessary to declare an edge as removed.
    /// The bool says which party is removing the edge: false for Peer0, true for Peer1
    /// The signature from the party removing the edge.
    removal_info: Option<(bool, Signature)>,
}

impl Edge {
    /// Create an addition edge.
    pub fn new(
        peer0: PeerId,
        peer1: PeerId,
        nonce: u64,
        signature0: Signature,
        signature1: Signature,
    ) -> Self {
        let (peer0, signature0, peer1, signature1) = if peer0 < peer1 {
            (peer0, signature0, peer1, signature1)
        } else {
            (peer1, signature1, peer0, signature0)
        };

        Self { peer0, peer1, nonce, signature0, signature1, removal_info: None }
    }

    /// Build a new edge with given information from the other party.
    pub fn build_with_secret_key(
        peer0: PeerId,
        peer1: PeerId,
        nonce: u64,
        secret_key: &SecretKey,
        signature1: Signature,
    ) -> Self {
        let hash = if peer0 < peer1 {
            Edge::build_hash(&peer0, &peer1, nonce)
        } else {
            Edge::build_hash(&peer1, &peer0, nonce)
        };
        let signature0 = secret_key.sign(hash.as_ref());
        Edge::new(peer0, peer1, nonce, signature0, signature1)
    }

    /// Create the remove edge change from an added edge change.
    pub fn remove_edge(&self, me: PeerId, sk: &SecretKey) -> Self {
        assert_eq!(self.edge_type(), EdgeType::Added);
        let mut edge = self.clone();
        edge.nonce += 1;
        let me = edge.peer0 == me;
        let hash = edge.hash();
        let signature = sk.sign(hash.as_ref());
        edge.removal_info = Some((me, signature));
        edge
    }

    /// Build the hash of the edge given its content.
    /// It is important that peer0 < peer1 at this point.
    fn build_hash(peer0: &PeerId, peer1: &PeerId, nonce: u64) -> CryptoHash {
        let mut buffer = Vec::<u8>::new();
        let peer0: Vec<u8> = peer0.clone().into();
        buffer.extend_from_slice(peer0.as_slice());
        let peer1: Vec<u8> = peer1.clone().into();
        buffer.extend_from_slice(peer1.as_slice());
        buffer.write_u64::<LittleEndian>(nonce).unwrap();
        hash(buffer.as_slice())
    }

    fn hash(&self) -> CryptoHash {
        Edge::build_hash(&self.peer0, &self.peer1, self.nonce)
    }

    fn prev_hash(&self) -> CryptoHash {
        Edge::build_hash(&self.peer0, &self.peer1, self.nonce - 1)
    }

    pub fn verify(&self) -> bool {
        if self.peer0 > self.peer1 {
            return false;
        }

        match self.edge_type() {
            EdgeType::Added => {
                let data = self.hash();

                self.removal_info.is_none()
                    && self.signature0.verify(data.as_ref(), &self.peer0.public_key())
                    && self.signature1.verify(data.as_ref(), &self.peer1.public_key())
            }
            EdgeType::Removed => {
                // nonce should be an even positive number
                if self.nonce == 0 {
                    return false;
                }

                // Check referring added edge is valid.
                let add_hash = self.prev_hash();
                if !self.signature0.verify(add_hash.as_ref(), &self.peer0.public_key())
                    || !self.signature1.verify(add_hash.as_ref(), &self.peer1.public_key())
                {
                    return false;
                }

                if let Some((party, signature)) = &self.removal_info {
                    let peer = if *party { &self.peer0 } else { &self.peer1 };
                    let del_hash = self.hash();
                    signature.verify(del_hash.as_ref(), &peer.public_key())
                } else {
                    false
                }
            }
        }
    }

    pub fn key(peer0: PeerId, peer1: PeerId) -> (PeerId, PeerId) {
        if peer0 < peer1 {
            (peer0, peer1)
        } else {
            (peer1, peer0)
        }
    }

    /// Helper function when adding a new edge and we receive information from new potential peer
    /// to verify the signature.
    pub fn partial_verify(peer0: PeerId, peer1: PeerId, edge_info: &EdgeInfo) -> bool {
        let pk = peer1.public_key();
        let (peer0, peer1) = Edge::key(peer0, peer1);
        let data = Edge::build_hash(&peer0, &peer1, edge_info.nonce);
        edge_info.signature.verify(data.as_ref(), &pk)
    }

    fn get_pair(&self) -> (PeerId, PeerId) {
        (self.peer0.clone(), self.peer1.clone())
    }

    /// It will be considered as a new edge if the nonce is odd, otherwise it is canceling the
    /// previous edge.
    pub fn edge_type(&self) -> EdgeType {
        if self.nonce % 2 == 1 {
            EdgeType::Added
        } else {
            EdgeType::Removed
        }
    }

    /// Next nonce of valid addition edge.
    pub fn next_nonce(&self) -> u64 {
        if self.nonce % 2 == 1 {
            self.nonce + 2
        } else {
            self.nonce + 1
        }
    }

    pub fn contains_peer(&self, peer_id: &PeerId) -> bool {
        self.peer0 == *peer_id || self.peer1 == *peer_id
    }

    /// Find a peer id in this edge different from `me`.
    pub fn other(&self, me: &PeerId) -> Option<PeerId> {
        if self.peer0 == *me {
            Some(self.peer1.clone())
        } else if self.peer1 == *me {
            Some(self.peer0.clone())
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct RoutingTable {
    // TODO(MarX, #1363): Use cache and file storing to keep this information.
    /// PeerId associated for every known account id.
    pub account_peers: HashMap<AccountId, AnnounceAccount>,
    /// Active PeerId that are part of the shortest path to each PeerId.
    pub peer_forwarding: HashMap<PeerId, HashSet<PeerId>>,
    /// Store last update for known edges.
    pub edges_info: HashMap<(PeerId, PeerId), Edge>,
    /// Hash of messages that requires routing back to respective previous hop.
    pub route_back: CloneNone<SizedCache<CryptoHash, PeerId>>,
    /// Current view of the network. Nodes are Peers and edges are active connections.
    raw_graph: Graph,
    /// Number of times each active connection was used to route a message.
    /// If there are several options use route with minimum nonce.
    /// New routes are added with minimum nonce.
    route_nonce: HashMap<PeerId, usize>,
    /// Flag to know if there is state recalculation pending.
    recalculation_scheduled: Option<Instant>,
    /// Ping received by nonce. Used for testing only.
    ping_info: Option<HashMap<usize, Ping>>,
    /// Ping received by nonce. Used for testing only.
    pong_info: Option<HashMap<usize, Pong>>,
}

#[derive(Debug)]
pub enum FindRouteError {
    Disconnected,
    PeerNotFound,
    AccountNotFound,
    RouteBackNotFound,
}

impl RoutingTable {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            account_peers: HashMap::new(),
            peer_forwarding: HashMap::new(),
            edges_info: HashMap::new(),
            route_back: CloneNone::new(SizedCache::with_size(ROUTE_BACK_CACHE_SIZE)),
            raw_graph: Graph::new(peer_id),
            route_nonce: HashMap::new(),
            recalculation_scheduled: None,
            ping_info: None,
            pong_info: None,
        }
    }

    /// Find peer that is connected to `source` and belong to the shortest path
    /// from `source` to `peer_id`.
    pub fn find_route_from_peer_id(&mut self, peer_id: &PeerId) -> Result<PeerId, FindRouteError> {
        if let Some(routes) = self.peer_forwarding.get(&peer_id) {
            // Strategy similar to Round Robin. Select node with least nonce and send it. Increase its
            // nonce by one. Additionally if the difference between the highest and nonce and the lowest
            // nonce is greater than some threshold increase the lowest nonce to be at least
            // max nonce - threshold.

            let (min_v, max_v) = routes.iter().fold((None, None), |(min_v, max_v), peer_id| {
                let nonce = self.route_nonce.get(&peer_id).cloned().unwrap_or(0usize);
                let current = (nonce, peer_id.clone());
                if min_v.is_none() || current < *min_v.as_ref().unwrap() {
                    (Some(current), max_v)
                } else if max_v.is_none() || *max_v.as_ref().unwrap() < current {
                    (max_v, Some(current))
                } else {
                    (min_v, max_v)
                }
            });

            let next_hop = match (min_v, max_v) {
                (None, _) => {
                    return Err(FindRouteError::Disconnected);
                }
                (Some(min_v), None) => min_v.1,
                (Some(min_v), Some(max_v)) => {
                    if min_v.0 + ROUND_ROBIN_MAX_NONCE_DIFFERENCE_ALLOWED < max_v.0 {
                        self.route_nonce.insert(
                            min_v.1.clone(),
                            max_v.0 - ROUND_ROBIN_MAX_NONCE_DIFFERENCE_ALLOWED,
                        );
                    }
                    min_v.1
                }
            };

            self.route_nonce
                .entry(next_hop.clone())
                .and_modify(|nonce| {
                    *nonce += 1;
                })
                .or_insert(1);
            Ok(next_hop)
        } else {
            Err(FindRouteError::PeerNotFound)
        }
    }

    pub fn find_route(&mut self, target: &PeerIdOrHash) -> Result<PeerId, FindRouteError> {
        match target {
            PeerIdOrHash::PeerId(peer_id) => self.find_route_from_peer_id(&peer_id),
            PeerIdOrHash::Hash(hash) => {
                self.fetch_route_back(hash.clone()).ok_or(FindRouteError::RouteBackNotFound)
            }
        }
    }

    /// Find peer that owns this AccountId.
    pub fn account_owner(&self, account_id: &AccountId) -> Result<PeerId, FindRouteError> {
        self.account_peers
            .get(account_id)
            .map(|announce_account| announce_account.peer_id.clone())
            .ok_or_else(|| FindRouteError::AccountNotFound)
    }

    /// Add (account id, peer id) to routing table.
    /// Returns a bool indicating whether this is a new entry or not.
    /// Note: There is at most on peer id per account id.
    pub fn add_account(&mut self, announce_account: AnnounceAccount) -> bool {
        let account_id = announce_account.account_id.clone();
        self.account_peers
            .insert(account_id, announce_account.clone())
            .map_or(true, |old_announce_account| old_announce_account == announce_account)
    }

    pub fn contains_account(&self, announce_account: AnnounceAccount) -> bool {
        self.account_peers
            .get(&announce_account.account_id)
            .map_or(false, |cur_announce_account| *cur_announce_account == announce_account)
    }

    /// Add this edge to the current view of the network.
    /// This edge is assumed to be valid at this point.
    /// Edge contains about being added or removed (this can trigger both types of events).
    /// Return true if the edge contains new information about the network. Old if this information
    /// is outdated.
    pub fn process_edge(&mut self, edge: Edge) -> ProcessEdgeResult {
        let key = edge.get_pair();

        if self.find_nonce(&key) >= edge.nonce {
            // We already have a newer information about this edge. Discard this information.
            debug!(target:"network", "Received outdated edge: {:?}", edge);
            return ProcessEdgeResult { new_edge: false, schedule_computation: None };
        }

        match edge.edge_type() {
            EdgeType::Added => {
                self.raw_graph.add_edge(key.0.clone(), key.1.clone());
            }
            EdgeType::Removed => {
                self.raw_graph.remove_edge(&key.0, &key.1);
            }
        }

        self.edges_info.insert(key, edge);

        // Minimum between known routes and 1000
        let known_routes = std::cmp::min(self.peer_forwarding.len() as u64, 1000);

        let new_schedule = self.recalculation_scheduled.map_or_else(
            move || Some(Duration::from_millis(known_routes)),
            |target| {
                if Instant::now() > target {
                    Some(Duration::from_millis(known_routes))
                } else {
                    None
                }
            },
        );

        if let Some(duration) = new_schedule {
            self.recalculation_scheduled = Some(Instant::now() + duration);
        }

        ProcessEdgeResult { new_edge: true, schedule_computation: new_schedule }
    }

    pub fn find_nonce(&self, edge: &(PeerId, PeerId)) -> u64 {
        self.edges_info.get(&edge).map_or(0, |x| x.nonce)
    }

    pub fn get_edge(&self, peer0: PeerId, peer1: PeerId) -> Option<Edge> {
        let key = Edge::key(peer0, peer1);
        self.edges_info.get(&key).cloned()
    }

    pub fn get_edges(&self) -> Vec<Edge> {
        self.edges_info.iter().map(|(_, edge)| edge.clone()).collect()
    }

    pub fn get_accounts(&self) -> Vec<AnnounceAccount> {
        self.account_peers.iter().map(|(_key, value)| value.clone()).collect()
    }

    pub fn add_route_back(&mut self, hash: CryptoHash, peer_id: PeerId) {
        self.route_back.value().cache_set(hash, peer_id);
    }

    // Find route back with given hash and removes it from cache.
    fn fetch_route_back(&mut self, hash: CryptoHash) -> Option<PeerId> {
        self.route_back.value().cache_remove(&hash)
    }

    pub fn compare_route_back(&mut self, hash: CryptoHash, peer_id: &PeerId) -> bool {
        self.route_back.value().cache_get(&hash).map_or(false, |value| value == peer_id)
    }

    pub fn add_ping(&mut self, ping: Ping) {
        if self.ping_info.is_none() {
            self.ping_info = Some(HashMap::new());
        }

        if let Some(ping_info) = self.ping_info.as_mut() {
            ping_info.entry(ping.nonce).or_insert(ping);
        }
    }

    pub fn add_pong(&mut self, pong: Pong) {
        if self.pong_info.is_none() {
            self.pong_info = Some(HashMap::new());
        }

        if let Some(pong_info) = self.pong_info.as_mut() {
            pong_info.entry(pong.nonce).or_insert(pong);
        }
    }

    pub fn fetch_ping_pong(&self) -> (HashMap<usize, Ping>, HashMap<usize, Pong>) {
        let pings = self.ping_info.clone().unwrap_or_else(HashMap::new);
        let pongs = self.pong_info.clone().unwrap_or_else(HashMap::new);
        (pings, pongs)
    }

    pub fn info(&self) -> RoutingTableInfo {
        let account_peers = self
            .account_peers
            .iter()
            .map(|(key, value)| (key.clone(), value.peer_id.clone()))
            .collect();

        RoutingTableInfo { account_peers, peer_forwarding: self.peer_forwarding.clone() }
    }

    /// Recalculate routing table.
    pub fn update(&mut self) {
        trace!(target: "network", "Update routing table.");
        self.recalculation_scheduled = None;
        self.peer_forwarding = self.raw_graph.calculate_distance();
    }
}

pub struct ProcessEdgeResult {
    pub new_edge: bool,
    pub schedule_computation: Option<Duration>,
}

#[derive(Debug)]
pub struct RoutingTableInfo {
    pub account_peers: HashMap<AccountId, PeerId>,
    pub peer_forwarding: HashMap<PeerId, HashSet<PeerId>>,
}

#[derive(Clone)]
pub struct Graph {
    pub source: PeerId,
    adjacency: HashMap<PeerId, HashSet<PeerId>>,
}

impl Graph {
    pub fn new(source: PeerId) -> Self {
        Self { source, adjacency: HashMap::new() }
    }

    fn contains_edge(&mut self, peer0: &PeerId, peer1: &PeerId) -> bool {
        if let Some(adj) = self.adjacency.get(&peer0) {
            if adj.contains(&peer1) {
                return true;
            }
        }

        false
    }

    fn add_directed_edge(&mut self, peer0: PeerId, peer1: PeerId) {
        self.adjacency.entry(peer0).or_insert_with(HashSet::new).insert(peer1);
    }

    fn remove_directed_edge(&mut self, peer0: &PeerId, peer1: &PeerId) {
        self.adjacency.get_mut(&peer0).unwrap().remove(&peer1);
    }

    pub fn add_edge(&mut self, peer0: PeerId, peer1: PeerId) {
        if !self.contains_edge(&peer0, &peer1) {
            self.add_directed_edge(peer0.clone(), peer1.clone());
            self.add_directed_edge(peer1, peer0);
        }
    }

    pub fn remove_edge(&mut self, peer0: &PeerId, peer1: &PeerId) {
        if self.contains_edge(&peer0, &peer1) {
            self.remove_directed_edge(&peer0, &peer1);
            self.remove_directed_edge(&peer1, &peer0);
        }
    }

    // TODO(MarX, #1363): This is too slow right now. (See benchmarks)
    /// Compute for every node `u` on the graph (other than `source`) which are the neighbors of
    /// `sources` which belong to the shortest path from `source` to `u`. Nodes that are
    /// not connected to `source` will not appear in the result.
    pub fn calculate_distance(&self) -> HashMap<PeerId, HashSet<PeerId>> {
        let mut queue = vec![];
        let mut distance = HashMap::new();
        // TODO(MarX, #1363): Represent routes more efficiently at least while calculating distances
        let mut routes: HashMap<PeerId, HashSet<PeerId>> = HashMap::new();

        distance.insert(&self.source, 0);

        // Add active connections
        if let Some(neighbors) = self.adjacency.get(&self.source) {
            for neighbor in neighbors {
                queue.push(neighbor);
                distance.insert(neighbor, 1);
                routes.insert(neighbor.clone(), vec![neighbor.clone()].drain(..).collect());
            }
        }

        let mut head = 0;

        while head < queue.len() {
            let cur_peer = queue[head];
            let cur_distance = *distance.get(cur_peer).unwrap();
            head += 1;

            if let Some(neighbors) = self.adjacency.get(&cur_peer) {
                for neighbor in neighbors {
                    if !distance.contains_key(&neighbor) {
                        queue.push(neighbor);
                        distance.insert(neighbor, cur_distance + 1);
                        routes.insert(neighbor.clone(), HashSet::new());
                    }

                    // If this edge belong to a shortest path, all paths to
                    // the closer nodes are also valid for the current node.
                    if *distance.get(neighbor).unwrap() == cur_distance + 1 {
                        let adding_routes = routes.get(cur_peer).unwrap().clone();
                        let target_routes = routes.get_mut(neighbor).unwrap();

                        for route in adding_routes {
                            target_routes.insert(route.clone());
                        }
                    }
                }
            }
        }

        routes.into_iter().filter(|(_, hops)| !hops.is_empty()).collect()
    }
}

#[cfg(test)]
mod test {
    use crate::routing::Graph;
    use crate::test_utils::{expected_routing_tables, random_peer_id};

    #[test]
    fn graph_contains_edge() {
        let source = random_peer_id();

        let node0 = random_peer_id();
        let node1 = random_peer_id();

        let mut graph = Graph::new(source.clone());

        assert_eq!(graph.contains_edge(&source, &node0), false);
        assert_eq!(graph.contains_edge(&source, &node1), false);
        assert_eq!(graph.contains_edge(&node0, &node1), false);
        assert_eq!(graph.contains_edge(&node1, &node0), false);

        graph.add_edge(node0.clone(), node1.clone());

        assert_eq!(graph.contains_edge(&source, &node0), false);
        assert_eq!(graph.contains_edge(&source, &node1), false);
        assert_eq!(graph.contains_edge(&node0, &node1), true);
        assert_eq!(graph.contains_edge(&node1, &node0), true);

        graph.remove_edge(&node1, &node0);

        assert_eq!(graph.contains_edge(&node0, &node1), false);
        assert_eq!(graph.contains_edge(&node1, &node0), false);
    }

    #[test]
    fn graph_distance0() {
        let source = random_peer_id();
        let node0 = random_peer_id();

        let mut graph = Graph::new(source.clone());
        graph.add_edge(source.clone(), node0.clone());

        assert!(expected_routing_tables(
            graph.calculate_distance(),
            vec![(node0.clone(), vec![node0.clone()])],
        ));
    }

    #[test]
    fn graph_distance1() {
        let source = random_peer_id();
        let nodes: Vec<_> = (0..3).map(|_| random_peer_id()).collect();

        let mut graph = Graph::new(source.clone());

        graph.add_edge(nodes[0].clone(), nodes[1].clone());
        graph.add_edge(nodes[2].clone(), nodes[1].clone());
        graph.add_edge(nodes[1].clone(), nodes[2].clone());

        assert!(expected_routing_tables(graph.calculate_distance(), vec![]));
    }

    #[test]
    fn graph_distance2() {
        let source = random_peer_id();
        let nodes: Vec<_> = (0..3).map(|_| random_peer_id()).collect();

        let mut graph = Graph::new(source.clone());

        graph.add_edge(nodes[0].clone(), nodes[1].clone());
        graph.add_edge(nodes[2].clone(), nodes[1].clone());
        graph.add_edge(nodes[1].clone(), nodes[2].clone());
        graph.add_edge(source.clone(), nodes[0].clone());

        assert!(expected_routing_tables(
            graph.calculate_distance(),
            vec![
                (nodes[0].clone(), vec![nodes[0].clone()]),
                (nodes[1].clone(), vec![nodes[0].clone()]),
                (nodes[2].clone(), vec![nodes[0].clone()]),
            ],
        ));
    }

    #[test]
    fn graph_distance3() {
        let source = random_peer_id();
        let nodes: Vec<_> = (0..3).map(|_| random_peer_id()).collect();

        let mut graph = Graph::new(source.clone());

        graph.add_edge(nodes[0].clone(), nodes[1].clone());
        graph.add_edge(nodes[2].clone(), nodes[1].clone());
        graph.add_edge(nodes[0].clone(), nodes[2].clone());
        graph.add_edge(source.clone(), nodes[0].clone());
        graph.add_edge(source.clone(), nodes[1].clone());

        assert!(expected_routing_tables(
            graph.calculate_distance(),
            vec![
                (nodes[0].clone(), vec![nodes[0].clone()]),
                (nodes[1].clone(), vec![nodes[1].clone()]),
                (nodes[2].clone(), vec![nodes[0].clone(), nodes[1].clone()]),
            ],
        ));
    }

    /// Test the following graph
    ///     0 - 3 - 6
    ///   /   x   x
    /// s - 1 - 4 - 7
    ///   \   x   x
    ///     2 - 5 - 8
    ///
    ///    9 - 10 (Dummy edge disconnected)
    ///
    /// There is a shortest path to nodes [3..9) going through 0, 1, and 2.
    #[test]
    fn graph_distance4() {
        let source = random_peer_id();
        let nodes: Vec<_> = (0..11).map(|_| random_peer_id()).collect();

        let mut graph = Graph::new(source.clone());

        for i in 0..3 {
            graph.add_edge(source.clone(), nodes[i].clone());
        }

        for level in 0..2 {
            for i in 0..3 {
                for j in 0..3 {
                    graph.add_edge(nodes[level * 3 + i].clone(), nodes[level * 3 + 3 + j].clone());
                }
            }
        }

        // Dummy edge.
        graph.add_edge(nodes[9].clone(), nodes[10].clone());

        let mut next_hops: Vec<_> =
            (0..3).map(|i| (nodes[i].clone(), vec![nodes[i].clone()])).collect();
        let target: Vec<_> = (0..3).map(|i| nodes[i].clone()).collect();

        for i in 3..9 {
            next_hops.push((nodes[i].clone(), target.clone()));
        }

        assert!(expected_routing_tables(graph.calculate_distance(), next_hops));
    }
}