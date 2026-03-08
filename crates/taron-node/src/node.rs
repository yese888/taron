//! TaronNode — main P2P node orchestrating TCP gossip, mempool, peer management,
//! and the blockchain (chain of blocks).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock};
use taron_core::{Block, Blockchain, Transaction, Ledger, Wallet, TransactionStatus, TESTNET_DIFFICULTY};
use tracing::{debug, info, warn};

use crate::mempool::Mempool;
use crate::peer::{PeerDirection, PeerManager};
use crate::protocol::{self, Message};
use crate::seeds::resolve_seeds;
use crate::finality::NodeFinalityManager;

/// Default TCP listen port.
pub const DEFAULT_PORT: u16 = 8333;

/// Node configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub listen_port: u16,
    pub seed_nodes: Vec<SocketAddr>,
    pub enable_discovery: bool,
    /// Data directory for persistent storage (chain.json, ledger.json).
    /// If None, data is kept in memory only.
    pub data_dir: Option<PathBuf>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            listen_port: DEFAULT_PORT,
            seed_nodes: Vec::new(),
            enable_discovery: true,
            data_dir: None,
        }
    }
}

/// Node status snapshot (for CLI display).
#[derive(Debug, Clone)]
pub struct NodeStatus {
    pub listen_port: u16,
    pub peer_count: usize,
    pub inbound_count: usize,
    pub outbound_count: usize,
    pub mempool_size: usize,
    pub account_count: usize,
    pub total_supply: u64,
    pub chain_height: u64,
    /// Hex-encoded hash of the chain tip block.
    pub best_hash: String,
}

/// The main TARON P2P node.
#[derive(Clone)]
pub struct TaronNode {
    config: NodeConfig,
    pub mempool: Arc<RwLock<Mempool>>,
    pub peers: Arc<Mutex<PeerManager>>,
    /// Tracks which peers have which tx hashes (for gossip dedup).
    peer_known_txs: Arc<RwLock<HashSet<(SocketAddr, String)>>>,
    /// The ledger containing all account states.
    pub ledger: Arc<RwLock<Ledger>>,
    /// The blockchain (ordered chain of validated blocks).
    pub blockchain: Arc<RwLock<Blockchain>>,
    /// Finality manager for transaction ACKs and double-spend prevention.
    pub finality: Arc<NodeFinalityManager>,
    /// Data directory for persistence.
    data_dir: Option<PathBuf>,
}

impl TaronNode {
    pub fn new(config: NodeConfig) -> Self {
        // Generate a node wallet for signing ACKs
        let node_wallet = Wallet::generate();
        let finality = NodeFinalityManager::new(1, node_wallet);

        // Load from disk if data_dir is set
        let (blockchain, ledger) = if let Some(ref dir) = config.data_dir {
            std::fs::create_dir_all(dir).ok();
            let chain_path = dir.join("chain.db");
            let ledger_path = dir.join("ledger.bin");
            let chain = Blockchain::load_or_create(&chain_path, TESTNET_DIFFICULTY);
            let ledger = Ledger::load_or_create_testnet(&ledger_path, &chain);
            info!("Loaded state from disk — height: {}, accounts: {}", chain.height(), ledger.account_count());
            (chain, ledger)
        } else {
            {
                let tmp = std::env::temp_dir().join(format!("taron_node_{}", std::process::id()));
                let chain = Blockchain::load_or_create(&tmp, TESTNET_DIFFICULTY);
                (chain, Ledger::new())
            }
        };

        let data_dir = config.data_dir.clone();
        Self {
            config,
            mempool: Arc::new(RwLock::new(Mempool::new())),
            peers: Arc::new(Mutex::new(PeerManager::new())),
            peer_known_txs: Arc::new(RwLock::new(HashSet::new())),
            ledger: Arc::new(RwLock::new(ledger)),
            blockchain: Arc::new(RwLock::new(blockchain)),
            finality: Arc::new(finality),
            data_dir,
        }
    }

    /// Create a new testnet node with genesis state.
    pub fn new_testnet(config: NodeConfig) -> Self {
        let ledger = Ledger::new_testnet();
        // Generate a node wallet for signing ACKs
        let node_wallet = Wallet::generate();
        let finality = NodeFinalityManager::new(1, node_wallet);
        let data_dir = config.data_dir.clone();
        let chain_path = data_dir.as_ref()
            .map(|d| d.join("chain.db"))
            .unwrap_or_else(|| std::env::temp_dir().join(format!("taron_genesis_{}", std::process::id())));

        Self {
            config,
            mempool: Arc::new(RwLock::new(Mempool::new())),
            peers: Arc::new(Mutex::new(PeerManager::new())),
            peer_known_txs: Arc::new(RwLock::new(HashSet::new())),
            ledger: Arc::new(RwLock::new(ledger)),
            blockchain: Arc::new(RwLock::new(
                Blockchain::load_or_create(&chain_path, TESTNET_DIFFICULTY)
            )),
            finality: Arc::new(finality),
            data_dir,
        }
    }

    /// Save blockchain and ledger to disk (if data_dir is set).
    pub async fn save_state(&self) {
        if let Some(ref dir) = self.data_dir {
            let chain = self.blockchain.read().await;
            let ledger = self.ledger.read().await;
            chain.save(&dir.join("chain.db"));
            ledger.save(&dir.join("ledger.bin"));
        }
    }

    /// Get current node status.
    pub async fn status(&self) -> NodeStatus {
        let peers = self.peers.lock().await;
        let mempool = self.mempool.read().await;
        let ledger = self.ledger.read().await;
        let blockchain = self.blockchain.read().await;
        NodeStatus {
            listen_port: self.config.listen_port,
            peer_count: peers.count(),
            inbound_count: peers.inbound_count(),
            outbound_count: peers.outbound_count(),
            mempool_size: mempool.len(),
            account_count: ledger.account_count(),
            total_supply: ledger.total_supply(),
            chain_height: blockchain.height(),
            best_hash: hex::encode(blockchain.tip().hash),
        }
    }

    /// Start the node: listen for connections and connect to seed nodes.
    pub async fn run(&self) -> io::Result<()> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.config.listen_port)).await?;
        info!("TARON node listening on port {}", self.config.listen_port);

        // Connect to seed nodes — config seeds take priority; fall back to TESTNET_SEEDS.
        let seeds = resolve_seeds(&self.config.seed_nodes);
        for seed in seeds {
            let mempool = self.mempool.clone();
            let peers = self.peers.clone();
            let known = self.peer_known_txs.clone();
            let ledger = self.ledger.clone();
            let blockchain = self.blockchain.clone();
            let finality = self.finality.clone();
            let port = self.config.listen_port;
            let data_dir = self.data_dir.clone();
            tokio::spawn(async move {
                if let Err(e) = connect_to_peer(seed, port, mempool, peers, known, ledger, blockchain, finality, data_dir).await {
                    warn!("Failed to connect to seed {}: {}", seed, e);
                }
            });
        }

        // Start UDP discovery if enabled
        if self.config.enable_discovery {
            let port = self.config.listen_port;
            tokio::spawn(async move {
                if let Err(e) = crate::discovery::run_discovery_listener(port).await {
                    warn!("Discovery listener error: {}", e);
                }
            });
        }

        // Accept incoming connections
        loop {
            let (stream, addr) = listener.accept().await?;
            info!("Incoming connection from {}", addr);

            let can_accept = {
                let mut peers = self.peers.lock().await;
                !peers.is_banned(addr.ip()) && peers.can_accept(PeerDirection::Inbound)
            };

            if !can_accept {
                debug!("Rejecting {}: inbound limit reached or IP banned", addr);
                drop(stream);
                continue;
            }

            {
                let mut peers = self.peers.lock().await;
                peers.add_peer(addr, PeerDirection::Inbound);
            }

            let mempool = self.mempool.clone();
            let peers = self.peers.clone();
            let known = self.peer_known_txs.clone();
            let ledger = self.ledger.clone();
            let blockchain = self.blockchain.clone();
            let finality = self.finality.clone();
            let port = self.config.listen_port;
            let data_dir = self.data_dir.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_peer(stream, addr, port, mempool, peers.clone(), known, ledger, blockchain, finality, data_dir).await {
                    debug!("Peer {} disconnected: {}", addr, e);
                }
                peers.lock().await.remove_peer(&addr);
            });
        }
    }

    /// Broadcast a transaction to all connected peers.
    pub async fn broadcast_tx(&self, tx: &Transaction) {
        let tx_hash = tx.hash_hex();
        let peer_addrs = self.peers.lock().await.all_addrs();

        for addr in peer_addrs {
            self.peer_known_txs.write().await.insert((addr, tx_hash.clone()));

            let tx = tx.clone();
            tokio::spawn(async move {
                if let Ok(mut stream) = TcpStream::connect(addr).await {
                    let _ = protocol::send_message(&mut stream, &Message::Tx { tx }).await;
                }
            });
        }
    }

    /// Broadcast a newly-mined block to all connected peers.
    /// Validate and apply a block submitted externally (e.g. from the pool server),
    /// then broadcast it to peers. Returns true if accepted.
    pub async fn submit_mined_block(&self, block: Block) -> bool {
        let mut chain = self.blockchain.write().await;
        let mut ledger = self.ledger.write().await;
        match chain.apply_block(&block, &mut ledger) {
            Ok(()) => {
                drop(chain);
                drop(ledger);
                self.save_state().await;
                // Purge included txs from mempool
                {
                    let mut mp = self.mempool.write().await;
                    for tx in &block.transactions {
                        mp.remove(&tx.hash_hex());
                    }
                }
                self.broadcast_block(&block).await;
                info!("Pool block #{} accepted and broadcast", block.index);
                true
            }
            Err(e) => {
                info!("Pool block #{} rejected: {:?}", block.index, e);
                false
            }
        }
    }

    pub async fn broadcast_block(&self, block: &Block) {
        let block_hash = hex::encode(&block.hash[..8]);
        let peer_addrs = self.peers.lock().await.all_addrs();

        info!(
            "[BROADCAST] Block #{} hash: {}… → {} peers",
            block.index,
            block_hash,
            peer_addrs.len()
        );

        for addr in peer_addrs {
            let block = block.clone();
            tokio::spawn(async move {
                if let Ok(mut stream) = TcpStream::connect(addr).await {
                    let _ = protocol::send_message(&mut stream, &Message::NewBlock(block)).await;
                }
            });
        }
    }
}

/// Connect to a peer as outbound.
async fn connect_to_peer(
    addr: SocketAddr,
    our_port: u16,
    mempool: Arc<RwLock<Mempool>>,
    peers: Arc<Mutex<PeerManager>>,
    known: Arc<RwLock<HashSet<(SocketAddr, String)>>>,
    ledger: Arc<RwLock<Ledger>>,
    blockchain: Arc<RwLock<Blockchain>>,
    finality: Arc<NodeFinalityManager>,
    data_dir: Option<PathBuf>,
) -> io::Result<()> {
    {
        let mut pm = peers.lock().await;
        if !pm.add_peer(addr, PeerDirection::Outbound) {
            return Ok(());
        }
    }

    let mut stream = TcpStream::connect(addr).await?;
    info!("Connected to peer {}", addr);

    // Send Hello
    protocol::send_message(&mut stream, &Message::Hello {
        version: 1,
        listen_port: our_port,
        user_agent: "taron/0.2.0".into(),
    }).await?;

    // Chain-sync handshake: ask peer for their chain height (triggers IBD if needed)
    protocol::send_message(&mut stream, &Message::GetChainHeight).await?;

    // State sync: request tx hashes
    protocol::send_message(&mut stream, &Message::GetTxHashes).await?;

    // Handle messages
    let result = handle_messages(&mut stream, addr, our_port, mempool, peers.clone(), known, ledger, blockchain, finality, data_dir).await;
    peers.lock().await.remove_peer(&addr);
    result
}

/// Handle an accepted peer connection.
async fn handle_peer(
    mut stream: TcpStream,
    addr: SocketAddr,
    our_port: u16,
    mempool: Arc<RwLock<Mempool>>,
    peers: Arc<Mutex<PeerManager>>,
    known: Arc<RwLock<HashSet<(SocketAddr, String)>>>,
    ledger: Arc<RwLock<Ledger>>,
    blockchain: Arc<RwLock<Blockchain>>,
    finality: Arc<NodeFinalityManager>,
    data_dir: Option<PathBuf>,
) -> io::Result<()> {
    // Send Hello
    protocol::send_message(&mut stream, &Message::Hello {
        version: 1,
        listen_port: our_port,
        user_agent: "taron/0.2.0".into(),
    }).await?;

    // Announce our height so the inbound peer can sync from us if needed
    protocol::send_message(&mut stream, &Message::GetChainHeight).await?;

    handle_messages(&mut stream, addr, our_port, mempool, peers, known, ledger, blockchain, finality, data_dir).await
}

/// Message processing loop for a peer connection.
async fn handle_messages(
    stream: &mut TcpStream,
    addr: SocketAddr,
    _our_port: u16,
    mempool: Arc<RwLock<Mempool>>,
    peers: Arc<Mutex<PeerManager>>,
    known: Arc<RwLock<HashSet<(SocketAddr, String)>>>,
    ledger: Arc<RwLock<Ledger>>,
    blockchain: Arc<RwLock<Blockchain>>,
    finality: Arc<NodeFinalityManager>,
    data_dir: Option<PathBuf>,
) -> io::Result<()> {
    // Track the peer's reported chain height so IBD can continue chunk by chunk.
    let mut peer_height: Option<u64> = None;

    loop {
        let msg = protocol::recv_message(stream).await?;

        match msg {
            Message::Hello { version, user_agent, .. } => {
                peers.lock().await.update_hello(&addr, version, user_agent);
            }

            Message::Ping { nonce } => {
                protocol::send_message(stream, &Message::Pong { nonce }).await?;
                peers.lock().await.touch(&addr);
            }

            Message::Pong { .. } => {
                peers.lock().await.touch(&addr);
            }

            Message::GetPeers => {
                let addrs = peers.lock().await.all_addrs();
                protocol::send_message(stream, &Message::Peers { addrs }).await?;
            }

            Message::Peers { .. } => {
                // Could add new peers here; for now just acknowledge
            }

            Message::Tx { tx } => {
                let tx_hash = tx.hash();
                let tx_hash_hex = tx.hash_hex();

                // Check for double-spend via sequence number
                if let Some(original) = finality.check_double_spend(&tx).await {
                    warn!(
                        "[DOUBLE-SPEND] tx {} rejected — conflicts with {}",
                        &tx_hash_hex[..16], hex::encode(&original[..8])
                    );
                    finality.reject(tx_hash, "double-spend".into()).await;
                    continue;
                }

                // Validate against current ledger state
                let ledger_validation = {
                    let ledger_state = ledger.read().await;
                    let mut ledger_copy = ledger_state.clone();
                    ledger_copy.apply_tx(&tx)
                };

                match ledger_validation {
                    Ok(()) => {
                        let mut pool = mempool.write().await;
                        match pool.insert(tx.clone()) {
                            Ok(true) => {
                                info!("[TX] {} from peer {} — validated ✓", &tx_hash_hex[..16], addr);

                                // Record for double-spend prevention
                                finality.record_seen(&tx).await;

                                {
                                    let mut ledger_state = ledger.write().await;
                                    if let Err(e) = ledger_state.apply_tx(&tx) {
                                        warn!("Ledger application failed for {}: {}", tx_hash_hex, e);
                                        drop(ledger_state);
                                        return Ok(());
                                    }
                                }

                                known.write().await.insert((addr, tx_hash_hex.clone()));

                                // Send ACK back to originator
                                let ack = finality.create_ack(tx_hash);
                                protocol::send_message(stream, &Message::TxAck(ack.clone())).await?;
                                debug!("[ACK] Sent ACK for {} to {}", &tx_hash_hex[..16], addr);

                                // Relay tx and ACK to other peers
                                let other_peers: Vec<SocketAddr> = {
                                    let pm = peers.lock().await;
                                    pm.all_addrs().into_iter().filter(|a| *a != addr).collect()
                                };
                                drop(pool);

                                for peer_addr in other_peers {
                                    let already_known = known.read().await.contains(&(peer_addr, tx_hash_hex.clone()));
                                    if !already_known {
                                        known.write().await.insert((peer_addr, tx_hash_hex.clone()));
                                        let tx = tx.clone();
                                        let ack = ack.clone();
                                        tokio::spawn(async move {
                                            if let Ok(mut s) = TcpStream::connect(peer_addr).await {
                                                let _ = protocol::send_message(&mut s, &Message::Tx { tx }).await;
                                                let _ = protocol::send_message(&mut s, &Message::TxAck(ack)).await;
                                            }
                                        });
                                    }
                                }
                            }
                            Ok(false) => {
                                debug!("Duplicate tx {} from {}", tx_hash_hex, addr);
                            }
                            Err(e) => {
                                warn!("Mempool validation failed for tx {} from {}: {}", tx_hash_hex, addr, e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Ledger validation failed for tx {} from {}: {}", tx_hash_hex, addr, e);
                    }
                }
            }

            Message::GetTxHashes => {
                let hashes = mempool.read().await.tx_hashes();
                protocol::send_message(stream, &Message::TxHashes { hashes }).await?;
            }

            Message::TxHashes { hashes } => {
                let missing: Vec<String> = {
                    let pool = mempool.read().await;
                    hashes.into_iter().filter(|h| !pool.contains(h)).collect()
                };
                if !missing.is_empty() {
                    protocol::send_message(stream, &Message::GetTxs { hashes: missing }).await?;
                }
            }

            Message::GetTxs { hashes } => {
                let pool = mempool.read().await;
                let txs = pool.get_txs(&hashes);
                for tx in txs {
                    protocol::send_message(stream, &Message::Tx { tx }).await?;
                }
            }

            // ── Block propagation ────────────────────────────────────────────

            Message::NewBlock(block) => {
                let block_index = block.index;
                let block_hash_hex = hex::encode(&block.hash[..8]);

                let result = {
                    let mut chain = blockchain.write().await;
                    let mut ledger_state = ledger.write().await;
                    chain.apply_block(&block, &mut *ledger_state)
                };

                match result {
                    Ok(()) => {
                        info!(
                            "[BLOCK] #{} accepted from {} | hash: {}… | reward: {:.2} TAR",
                            block_index,
                            addr,
                            block_hash_hex,
                            block.reward as f64 / 1_000_000.0
                        );

                        // Purge included txs from mempool
                        {
                            let mut mp = mempool.write().await;
                            for tx in &block.transactions {
                                mp.remove(&tx.hash_hex());
                            }
                        }

                        // Persist to disk
                        if let Some(ref dir) = data_dir {
                            let chain = blockchain.read().await;
                            let ledger_state = ledger.read().await;
                            chain.save(&dir.join("chain.db"));
                            ledger_state.save(&dir.join("ledger.bin"));
                        }

                        // Relay block to other connected peers
                        let other_peers: Vec<SocketAddr> = {
                            let pm = peers.lock().await;
                            pm.all_addrs().into_iter().filter(|a| *a != addr).collect()
                        };

                        for peer_addr in other_peers {
                            let block = block.clone();
                            tokio::spawn(async move {
                                if let Ok(mut s) = TcpStream::connect(peer_addr).await {
                                    let _ = protocol::send_message(&mut s, &Message::NewBlock(block)).await;
                                }
                            });
                        }
                    }
                    Err(e) => {
                        let our_h = blockchain.read().await.height();
                        if block_index > our_h + 1 {
                            // We're behind — request all missing blocks from this peer
                            peer_height = Some(block_index);
                            info!(
                                "[SYNC] NewBlock #{} from {} is ahead of our height {} — requesting blocks {}..{}",
                                block_index, addr, our_h, our_h + 1, block_index
                            );
                            let from = our_h + 1;
                            let to = (from + crate::sync::IBD_CHUNK_SIZE - 1).min(block_index);
                            protocol::send_message(
                                stream,
                                &Message::GetBlocks { from, to },
                            ).await?;
                        } else {
                            warn!(
                                "[BLOCK] #{} rejected from {} ({})",
                                block_index, addr, e
                            );
                            // Penalize peer for sending an invalid block.
                            let banned = peers.lock().await
                                .penalize(&addr, crate::peer::PENALTY_INVALID_BLOCK);
                            if banned {
                                return Err(io::Error::new(
                                    io::ErrorKind::Other, "peer banned"
                                ));
                            }
                        }
                    }
                }
            }

            Message::GetChainHeight => {
                let height = blockchain.read().await.height();
                protocol::send_message(stream, &Message::ChainHeight(height)).await?;
            }

            Message::ChainHeight(peer_h) => {
                peer_height = Some(peer_h);
                let our_h = blockchain.read().await.height();
                if peer_h > our_h {
                    info!(
                        "[SYNC] Peer {} reports height {} — we are at {} — launching IBD",
                        addr, peer_h, our_h
                    );
                    let from = our_h + 1;
                    let to = (from + crate::sync::IBD_CHUNK_SIZE - 1).min(peer_h);
                    info!("[SYNC] Downloading blocks {}..{} from {}", from, to, addr);
                    protocol::send_message(stream, &Message::GetBlocks { from, to }).await?;
                } else {
                    info!("[SYNC] Peer {} height {} — already in sync (height {})", addr, peer_h, our_h);
                }
            }

            Message::GetBlocks { from, to } => {
                let chain = blockchain.read().await;
                let blocks = chain.blocks_range(from, to);
                protocol::send_message(stream, &Message::Blocks(blocks)).await?;
            }

            Message::Blocks(blocks) => {
                // Batch block sync — apply each in order
                if blocks.is_empty() {
                    let h = blockchain.read().await.height();
                    info!("[SYNC] Sync complete — height: {}", h);
                } else {
                    let mut applied = 0usize;
                    let mut last_height = 0u64;
                    for block in &blocks {
                        let result = {
                            let mut chain = blockchain.write().await;
                            let mut ledger_state = ledger.write().await;
                            chain.apply_block_ibd(block, &mut *ledger_state)
                        };
                        match result {
                            Ok(()) => {
                                applied += 1;
                                last_height = block.index;
                                let hash_prefix = hex::encode(&block.hash[..5]);
                                info!("[SYNC] Applied block #{} | hash: {}…", block.index, hash_prefix);
                            }
                            Err(e) => {
                                warn!("[SYNC] Block #{} rejected: {}", block.index, e);
                                break;
                            }
                        }
                    }
                    if applied > 0 {
                        info!("[SYNC] Applied {} blocks from {} — height now: {}", applied, addr, last_height);

                        // Persist to disk after sync batch
                        if let Some(ref dir) = data_dir {
                            let chain = blockchain.read().await;
                            let ledger_state = ledger.read().await;
                            chain.save(&dir.join("chain.db"));
                            ledger_state.save(&dir.join("ledger.bin"));
                        }

                        // Continue IBD if peer has more blocks
                        let our_h = blockchain.read().await.height();
                        if let Some(ph) = peer_height {
                            if our_h < ph {
                                let from = our_h + 1;
                                let to = (from + crate::sync::IBD_CHUNK_SIZE - 1).min(ph);
                                info!("[SYNC] Continuing IBD: downloading blocks {}..{} from {}", from, to, addr);
                                protocol::send_message(stream, &Message::GetBlocks { from, to }).await?;
                            } else {
                                info!("[SYNC] IBD complete — height: {}", our_h);
                            }
                        }
                    }
                }
            }

            // ── Transaction finality messages ────────────────────────────────

            Message::TxAck(ack) => {
                let tx_hash_hex = ack.tx_hash_hex();
                
                // Record the ACK
                if let Some(status) = finality.record_ack(ack.clone()).await {
                    match &status {
                        TransactionStatus::Confirmed { acks, finality_ms } => {
                            info!(
                                "[FINALITY] tx {} CONFIRMED — {} ACKs, {}ms finality",
                                &tx_hash_hex[..16], acks, finality_ms
                            );
                        }
                        TransactionStatus::Pending { acks, quorum } => {
                            debug!(
                                "[FINALITY] tx {} — {}/{} ACKs",
                                &tx_hash_hex[..16], acks, quorum
                            );
                        }
                        _ => {}
                    }

                    // Relay ACK to other peers
                    let other_peers: Vec<SocketAddr> = {
                        let pm = peers.lock().await;
                        pm.all_addrs().into_iter().filter(|a| *a != addr).collect()
                    };

                    for peer_addr in other_peers {
                        let ack = ack.clone();
                        tokio::spawn(async move {
                            if let Ok(mut s) = TcpStream::connect(peer_addr).await {
                                let _ = protocol::send_message(&mut s, &Message::TxAck(ack)).await;
                            }
                        });
                    }
                }
            }

            Message::GetTxStatus { tx_hash } => {
                let hash_bytes: [u8; 32] = hex::decode(&tx_hash)
                    .ok()
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or([0u8; 32]);
                
                let (acks, is_final) = if let Some(status) = finality.get_status(&hash_bytes).await {
                    match status {
                        TransactionStatus::Confirmed { acks, .. } => (acks, true),
                        TransactionStatus::Pending { acks, .. } => (acks, false),
                        _ => (0, false),
                    }
                } else {
                    (0, false)
                };

                protocol::send_message(stream, &Message::TxStatus {
                    tx_hash,
                    acks,
                    is_final,
                }).await?;
            }

            Message::TxStatus { tx_hash, acks, is_final } => {
                debug!(
                    "[STATUS] tx {} — {} ACKs, final: {}",
                    &tx_hash[..16.min(tx_hash.len())], acks, is_final
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_config_default() {
        let config = NodeConfig::default();
        assert_eq!(config.listen_port, 8333);
        assert!(config.seed_nodes.is_empty());
        assert!(config.enable_discovery);
    }

    #[tokio::test]
    async fn test_node_status() {
        let node = TaronNode::new(NodeConfig::default());
        let status = node.status().await;
        assert_eq!(status.peer_count, 0);
        assert_eq!(status.mempool_size, 0);
        assert_eq!(status.chain_height, 0);
    }

    #[tokio::test]
    async fn test_node_mempool_insert() {
        let node = TaronNode::new(NodeConfig::default());
        let sender = taron_core::Wallet::generate();
        let recipient = taron_core::Wallet::generate();
        let tx = taron_core::TxBuilder::new(&sender)
            .recipient(recipient.public_key())
            .amount(1_000_000)
            .sequence(1)
            .build_and_prove()
            .unwrap();

        {
            let mut pool = node.mempool.write().await;
            assert!(pool.insert(tx).unwrap());
        }

        let status = node.status().await;
        assert_eq!(status.mempool_size, 1);
    }

    #[tokio::test]
    async fn test_two_nodes_handshake() {
        // Start node A
        let node_a = Arc::new(TaronNode::new(NodeConfig {
            listen_port: 0,
            seed_nodes: vec![],
            enable_discovery: false,
            data_dir: None,
        }));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_a = listener.local_addr().unwrap();

        let mempool_a = node_a.mempool.clone();
        let peers_a = node_a.peers.clone();

        // Accept one connection on node A
        let server = tokio::spawn(async move {
            let (mut stream, addr) = listener.accept().await.unwrap();
            {
                let mut pm = peers_a.lock().await;
                pm.add_peer(addr, PeerDirection::Inbound);
            }
            protocol::send_message(&mut stream, &Message::Hello {
                version: 1,
                listen_port: addr_a.port(),
                user_agent: "node-a".into(),
            }).await.unwrap();
            let msg = protocol::recv_message(&mut stream).await.unwrap();
            match msg {
                Message::Hello { user_agent, .. } => assert_eq!(user_agent, "node-b"),
                other => panic!("expected Hello, got {:?}", other),
            }
            mempool_a.read().await.len()
        });

        // Node B connects
        let mut stream_b = TcpStream::connect(addr_a).await.unwrap();
        let msg = protocol::recv_message(&mut stream_b).await.unwrap();
        match msg {
            Message::Hello { user_agent, .. } => assert_eq!(user_agent, "node-a"),
            other => panic!("expected Hello, got {:?}", other),
        }
        protocol::send_message(&mut stream_b, &Message::Hello {
            version: 1,
            listen_port: 9999,
            user_agent: "node-b".into(),
        }).await.unwrap();

        let pool_size = server.await.unwrap();
        assert_eq!(pool_size, 0);
    }
}
