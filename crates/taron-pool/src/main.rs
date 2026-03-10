//! TARON Mining Pool Server
//!
//! HTTP API on port 8083. Miners connect, get a block template with the pool's
//! pubkey as miner, submit shares. When a share meets full block difficulty the
//! pool broadcasts the block to the node and pays all contributing miners (99%)
//! via P2P transactions. Pool keeps 1% fee.
//!
//! Endpoints:
//!   GET  /pool/status           — pool stats
//!   GET  /pool/work?address=…   — current block template
//!   POST /pool/share            — submit a nonce
//!   GET  /pool/miners           — leaderboard (shares per miner)
//!   GET  /pool/miner?address=…  — per-miner stats (shares_24h, total_paid, hashrate)

mod db;
mod dedup;
mod scoring;
mod vardiff;

use db::{Db, HashrateSnapshot, PayoutRecord, ShareRecord};
use dedup::TachyonGuard;
use vardiff::VarDiffRegistry;

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderValue, Method, StatusCode},
    response::Json,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use taron_core::{
    Block, TxBuilder, Wallet, WalletFile,
    meets_difficulty,
    wallet::address_from_pubkey,
    TESTNET_REWARD,
};

// ── Constants ────────────────────────────────────────────────────────────────

/// Port the pool server listens on.
const POOL_PORT: u16 = 8083;
/// Node RPC base URL.
const NODE_URL: &str = "http://127.0.0.1:8082";
/// Share difficulty = block_difficulty - this value (miners submit ~16x more often).
const SHARE_DIFF_OFFSET: u32 = 4;
/// Pool fee: 1% (out of 1000).
const POOL_FEE_PERMILLE: u64 = 10;
/// How often (ms) the pool polls the node for a new block.
const POLL_INTERVAL_MS: u64 = 2_000;

// ── Pool state ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct BlockTemplate {
    index: u64,
    prev_hash: [u8; 32],
    #[allow(dead_code)]
    timestamp: u64,
    difficulty: u32,
    share_difficulty: u32,
}

#[derive(Default)]
struct Round {
    /// Share count per miner address this round.
    shares: HashMap<String, u64>,
    total_shares: u64,
}

struct PoolState {
    wallet: Wallet,
    template: RwLock<BlockTemplate>,
    round: RwLock<Round>,
    /// Timestamp (ms) when the current round started (reset on each new block).
    round_start_ms: RwLock<u64>,
    /// Next sequence number for payout transactions.
    #[allow(dead_code)]
    payout_sequence: RwLock<u64>,
    /// Total blocks found by the pool (all time).
    blocks_found: RwLock<u64>,
    /// Per-worker adaptive difficulty (TachyonVarDiff).
    vardiff: RwLock<VarDiffRegistry>,
    /// Duplicate share detector (TachyonGuard — 3 rotating buckets).
    dedup: RwLock<TachyonGuard>,
    /// PostgreSQL/TimescaleDB persistence (shares, payouts, hashrate snapshots).
    db: Arc<Db>,
}

type Pool = Arc<PoolState>;

// ── Wallet helpers ────────────────────────────────────────────────────────────

fn wallet_path() -> PathBuf {
    let mut p = dirs_home();
    p.push(".taron-testnet");
    p.push("pool-wallet.key");
    p
}

fn db_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://taron_pool:taron_pool@localhost/taron_pool".to_string())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/root"))
}

fn load_or_create_wallet() -> Wallet {
    let path = wallet_path();
    if path.exists() {
        let data = std::fs::read_to_string(&path).expect("read pool wallet");
        let wf: WalletFile = serde_json::from_str(&data).expect("parse pool wallet");
        let w = Wallet::from_hex(&wf.private_key_hex).expect("load pool wallet");
        info!("Pool wallet loaded — address: {}", w.address());
        w
    } else {
        let w = Wallet::generate();
        let wf = w.to_file();
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        std::fs::write(&path, serde_json::to_string_pretty(&wf).unwrap())
            .expect("write pool wallet");
        info!("Pool wallet created — address: {}", w.address());
        w
    }
}

// ── Node API client ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct NodeStatus {
    chain_height: u64,
    best_hash: String,
    difficulty: u32,
}

#[derive(Deserialize)]
struct NodeAccount {
    sequence: u64,
    #[allow(dead_code)]
    balance: u64,
}

async fn fetch_node_status(client: &reqwest::Client) -> Option<NodeStatus> {
    client.get(format!("{}/api/v1/status", NODE_URL))
        .send().await.ok()?
        .json().await.ok()
}

async fn fetch_pool_account(client: &reqwest::Client, address: &str) -> Option<NodeAccount> {
    client.get(format!("{}/api/v1/accounts/{}", NODE_URL, address))
        .send().await.ok()?
        .json().await.ok()
}

/// Fetch all pending transactions from the node mempool (full structs, for inclusion in blocks).
async fn fetch_mempool_txs(client: &reqwest::Client) -> Vec<taron_core::Transaction> {
    match client.get(format!("{}/api/v1/mempool/raw", NODE_URL)).send().await {
        Ok(r) => r.json::<Vec<taron_core::Transaction>>().await.unwrap_or_default(),
        Err(_) => vec![],
    }
}

async fn submit_block_to_node(client: &reqwest::Client, block: &Block) -> bool {
    #[derive(Deserialize)]
    struct SubmitResp { accepted: bool, message: String }

    match client.post(format!("{}/api/v1/submit_block", NODE_URL))
        .json(block).send().await
    {
        Ok(r) => {
            if let Ok(resp) = r.json::<SubmitResp>().await {
                info!("Block submission: accepted={} msg={}", resp.accepted, resp.message);
                resp.accepted
            } else { false }
        }
        Err(e) => { warn!("Block submission failed: {}", e); false }
    }
}

// ── Hashrate estimation ───────────────────────────────────────────────────────

/// Estimate hashrate from a slice of shares.
/// Each share represents on average `2^share_difficulty` hashes.
/// Uses the actual time span between first and last share for accuracy,
/// falling back to the provided window if there are fewer than 2 shares.
fn estimate_hashrate(shares: &[ShareRecord], share_difficulty: u32, window_ms: u64) -> f64 {
    if shares.is_empty() { return 0.0; }
    let hashes_per_share = (1u64 << share_difficulty.min(63)) as f64;
    let total_hashes = shares.len() as f64 * hashes_per_share;
    // Use actual time span between first and last share for a more stable estimate.
    let span_ms = if shares.len() >= 2 {
        let first = shares.iter().map(|s| s.timestamp_ms).min().unwrap_or(0);
        let last = shares.iter().map(|s| s.timestamp_ms).max().unwrap_or(0);
        let span = last.saturating_sub(first);
        if span > 0 { span } else { window_ms }
    } else {
        window_ms
    };
    let span_s = span_ms as f64 / 1000.0;
    if span_s > 0.0 { total_hashes / span_s } else { 0.0 }
}

// ── Payout logic ─────────────────────────────────────────────────────────────

/// Build signed payout transactions for all miners in the current round.
/// Transactions are embedded directly in the found block (not submitted to mempool),
/// so they are confirmed atomically with the block — no mempool accumulation.
async fn build_payout_txs(
    pool: &PoolState,
    client: &reqwest::Client,
    block_index: u64,
    round_start_ms: u64,
) -> Vec<taron_core::Transaction> {
    let mut txs = vec![];

    // Snapshot round share counts for proportional payout computation.
    let (total_shares, shares_map) = {
        let round = pool.round.read().await;
        if round.total_shares == 0 { return txs; }
        (round.total_shares, round.shares.clone())
    };

    let to_distribute = TESTNET_REWARD - (TESTNET_REWARD * POOL_FEE_PERMILLE / 1000);

    // Look up pubkeys from DB.
    let pubkeys = match pool.db.miner_pubkeys_in_round(round_start_ms).await {
        Ok(p) => p,
        Err(e) => { warn!("Failed to get miner pubkeys: {}", e); return txs; }
    };

    // Fetch pool account for current sequence.
    let pool_address = pool.wallet.address();
    let account = fetch_pool_account(client, &pool_address).await
        .unwrap_or(NodeAccount { sequence: 0, balance: 0 });

    let mut sequence = account.sequence + 1;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;

    for (miner_address, miner_shares) in &shares_map {
        let payout = to_distribute * miner_shares / total_shares;
        if payout == 0 { continue; }

        let pubkey_hex = match pubkeys.get(miner_address) {
            Some(p) => p,
            None => { warn!("No pubkey for miner {} — skipping payout", miner_address); continue; }
        };
        let pubkey_bytes: [u8; 32] = match hex::decode(pubkey_hex) {
            Ok(b) if b.len() == 32 => { let mut arr = [0u8; 32]; arr.copy_from_slice(&b); arr }
            _ => { warn!("Invalid pubkey for {} — skipping payout", miner_address); continue; }
        };

        match TxBuilder::new(&pool.wallet)
            .recipient(pubkey_bytes)
            .amount(payout)
            .sequence(sequence)
            .build_and_prove()
        {
            Ok(tx) => {
                let tx_hash = hex::encode(tx.hash());
                info!("PSWA payout {} µTAR → {} (seq {}, tx {})", payout, miner_address, sequence, tx_hash);
                let rec = PayoutRecord {
                    timestamp_ms: now_ms,
                    to_address: miner_address.clone(),
                    amount_micro: payout,
                    tx_hash,
                    block_index,
                };
                if let Err(e) = pool.db.insert_payout(&rec).await {
                    warn!("Failed to record payout: {}", e);
                }
                txs.push(tx);
                sequence += 1;
            }
            Err(e) => { warn!("Failed to build tx for {}: {}", miner_address, e); }
        }
    }

    info!("Built {} payout txs for block #{}", txs.len(), block_index);
    txs
}

// ── Hashrate snapshot task ────────────────────────────────────────────────────

async fn snapshot_task(pool: Pool, db: Arc<Db>) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300));
    loop {
        interval.tick().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default()
            .as_millis() as u64;

        let tmpl = pool.template.read().await;
        let share_difficulty = tmpl.share_difficulty;
        drop(tmpl);

        let ten_min_ago = now.saturating_sub(600_000);
        let shares = db.shares_since(ten_min_ago).await.unwrap_or_default();
        let hashrate = estimate_hashrate(&shares, share_difficulty, 600_000);

        let round = pool.round.read().await;
        let active_miners = round.shares.len() as u32;
        drop(round);

        let snap = HashrateSnapshot { timestamp_ms: now, hashrate_hps: hashrate, active_miners };
        if let Err(e) = db.insert_hashrate_snapshot(&snap).await {
            warn!("Failed to insert hashrate snapshot: {}", e);
        }
    }
}

// ── Pool background task ──────────────────────────────────────────────────────

async fn pool_watcher(pool: Pool) {
    let client = reqwest::Client::new();
    let mut last_height: u64 = 0;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(POLL_INTERVAL_MS)).await;

        let status = match fetch_node_status(&client).await {
            Some(s) => s,
            None => { warn!("Cannot reach node"); continue; }
        };

        let pool_pubkey = pool.wallet.public_key();

        if status.chain_height > last_height {
            // New block on chain — update template, check if it was ours
            let new_tmpl = BlockTemplate {
                index: status.chain_height + 1,
                prev_hash: {
                    let bytes = hex::decode(&status.best_hash).unwrap_or_default();
                    let mut arr = [0u8; 32];
                    let l = bytes.len().min(32);
                    arr[..l].copy_from_slice(&bytes[..l]);
                    arr
                },
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH).unwrap_or_default()
                    .as_millis() as u64,
                difficulty: status.difficulty,
                share_difficulty: status.difficulty.saturating_sub(SHARE_DIFF_OFFSET),
            };

            info!(
                "New block #{} — updating template to #{}, diff={}",
                status.chain_height, new_tmpl.index, new_tmpl.difficulty
            );

            let new_diff = new_tmpl.difficulty;
            *pool.template.write().await = new_tmpl;

            // Reset round (shares count + PSWA weighted) and record start time.
            {
                let mut round = pool.round.write().await;
                *round = Round::default();
            }
            // Update VarDiff base difficulty when chain difficulty changes.
            {
                let mut vd = pool.vardiff.write().await;
                vd.set_base_difficulty(new_diff.saturating_sub(SHARE_DIFF_OFFSET));
            }
            {
                let now_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH).unwrap_or_default()
                    .as_millis() as u64;
                *pool.round_start_ms.write().await = now_ms;
            }

            last_height = status.chain_height;
            let _ = pool_pubkey;
        }
    }
}

// ── HTTP handlers ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PoolStatusResponse {
    pool_address: String,
    pool_pubkey: String,
    current_block: u64,
    difficulty: u32,
    share_difficulty: u32,
    active_miners: usize,
    total_shares_this_round: u64,
    blocks_found: u64,
    fee_percent: f64,
}

async fn get_pool_status(State(pool): State<Pool>) -> Json<PoolStatusResponse> {
    let tmpl = pool.template.read().await;
    let round = pool.round.read().await;
    let blocks_found = *pool.blocks_found.read().await;

    // Count active miners by recent share activity (last 2 minutes), not round state.
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let since_2m = now_ms.saturating_sub(2 * 60 * 1000);
    let active = pool.db.distinct_miners_since(since_2m).await.unwrap_or(0);

    Json(PoolStatusResponse {
        pool_address: pool.wallet.address(),
        pool_pubkey: hex::encode(pool.wallet.public_key()),
        current_block: tmpl.index,
        difficulty: tmpl.difficulty,
        share_difficulty: tmpl.share_difficulty,
        active_miners: active,
        total_shares_this_round: round.total_shares,
        blocks_found,
        fee_percent: (POOL_FEE_PERMILLE as f64) / 10.0,
    })
}

#[derive(Deserialize)]
struct WorkQuery {
    address: Option<String>,
    worker: Option<String>,
}

#[derive(Serialize)]
struct WorkResponse {
    block_index: u64,
    prev_hash: String,
    timestamp: u64,
    pool_pubkey: String,
    difficulty: u32,
    share_difficulty: u32,
    reward: u64,
}

async fn get_work(State(pool): State<Pool>, Query(q): Query<WorkQuery>) -> Json<WorkResponse> {
    let tmpl = pool.template.read().await;
    let block_index = tmpl.index;
    let prev_hash = tmpl.prev_hash;
    let difficulty = tmpl.difficulty;
    let floor_share_diff = tmpl.share_difficulty;
    drop(tmpl);

    // Refresh timestamp on every work request so miners don't get stale.
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;

    // Per-worker VarDiff share difficulty.
    let worker_key = format!(
        "{}:{}",
        q.address.as_deref().unwrap_or("unknown"),
        q.worker.as_deref().unwrap_or("default"),
    );
    let share_difficulty = {
        let mut vd = pool.vardiff.write().await;
        let vd_diff = vd.get_difficulty(&worker_key);
        // Never send a share_difficulty below the pool floor — otherwise the miner
        // submits shares that pass vardiff locally but get rejected by the pool.
        vd_diff.max(floor_share_diff)
    };

    if let Some(addr) = &q.address {
        info!("Work requested by {} worker={} share_diff={}", addr,
            q.worker.as_deref().unwrap_or("default"), share_difficulty);
    }

    Json(WorkResponse {
        block_index,
        prev_hash: hex::encode(prev_hash),
        timestamp,
        pool_pubkey: hex::encode(pool.wallet.public_key()),
        difficulty,
        share_difficulty,
        reward: TESTNET_REWARD,
    })
}

#[derive(Deserialize)]
struct ShareRequest {
    /// Miner's tar1... address (for payout attribution).
    miner_address: String,
    /// Miner's public key hex (needed to send payout tx).
    miner_pubkey: String,
    /// Worker name (identifies this rig/machine). Defaults to "default".
    #[serde(default = "default_worker_name")]
    worker_name: String,
    /// The nonce found by the miner.
    nonce: u64,
    /// Block index this share is for.
    block_index: u64,
    /// Timestamp used when hashing (must match what was in the work response).
    timestamp: u64,
    /// Prev hash used (must match current template).
    prev_hash: String,
}

fn default_worker_name() -> String { "default".to_string() }

#[derive(Serialize)]
struct ShareResponse {
    accepted: bool,
    is_block: bool,
    message: String,
}

async fn submit_share(
    State(pool): State<Pool>,
    Json(req): Json<ShareRequest>,
) -> (StatusCode, Json<ShareResponse>) {
    let tmpl = pool.template.read().await;

    // Verify miner_pubkey is consistent with miner_address — prevents payout hijacking.
    let pubkey_bytes: [u8; 32] = match hex::decode(&req.miner_pubkey) {
        Ok(b) if b.len() == 32 => { let mut arr = [0u8; 32]; arr.copy_from_slice(&b); arr }
        _ => return (StatusCode::BAD_REQUEST, Json(ShareResponse {
            accepted: false, is_block: false,
            message: "Invalid miner_pubkey format".into(),
        })),
    };
    if address_from_pubkey(&pubkey_bytes) != req.miner_address {
        return (StatusCode::BAD_REQUEST, Json(ShareResponse {
            accepted: false, is_block: false,
            message: "miner_address does not match miner_pubkey".into(),
        }));
    }

    // Basic sanity checks
    if req.block_index != tmpl.index {
        return (StatusCode::BAD_REQUEST, Json(ShareResponse {
            accepted: false, is_block: false,
            message: format!("Stale share: expected block #{}, got #{}", tmpl.index, req.block_index),
        }));
    }

    let expected_prev = hex::encode(tmpl.prev_hash);
    if req.prev_hash != expected_prev {
        return (StatusCode::BAD_REQUEST, Json(ShareResponse {
            accepted: false, is_block: false,
            message: "Stale share: prev_hash mismatch".into(),
        }));
    }

    // Build candidate block to compute hash
    let prev_hash_bytes = hex::decode(&req.prev_hash).unwrap_or_default();
    let mut prev_hash = [0u8; 32];
    let l = prev_hash_bytes.len().min(32);
    prev_hash[..l].copy_from_slice(&prev_hash_bytes[..l]);

    let pool_pubkey = pool.wallet.public_key();
    let candidate = Block {
        index: req.block_index,
        prev_hash,
        timestamp: req.timestamp,
        miner: pool_pubkey,
        nonce: req.nonce,
        hash: [0u8; 32],
        reward: TESTNET_REWARD,
        transactions: vec![],
    };

    let hash = candidate.hash_header();

    // Per-worker VarDiff: look up the difficulty assigned to this worker.
    let worker_key = format!("{}:{}", req.miner_address, req.worker_name);

    // TachyonGuard: reject duplicate (block_index, nonce, worker) triples.
    {
        let mut guard = pool.dedup.write().await;
        if guard.is_duplicate(req.block_index, req.nonce, &worker_key) {
            return (StatusCode::BAD_REQUEST, Json(ShareResponse {
                accepted: false, is_block: false,
                message: "Duplicate share rejected".into(),
            }));
        }
    }

    // Accept any share meeting the global pool floor (tmpl.share_difficulty).
    // VarDiff only adjusts what we tell miners to target in get_work — it does
    // not tighten the acceptance threshold (that would cause legitimate shares
    // submitted with stale work to be rejected).
    let floor_diff = tmpl.share_difficulty;
    if !meets_difficulty(&hash, floor_diff) {
        return (StatusCode::BAD_REQUEST, Json(ShareResponse {
            accepted: false, is_block: false,
            message: format!("Share does not meet minimum difficulty {}", floor_diff),
        }));
    }

    // Compute the actual leading-zero count for PSWA weighting.
    // Use the assigned VarDiff difficulty as the base for weight calculation.
    let assigned_share_diff = {
        let mut vd = pool.vardiff.write().await;
        // on_share updates EWIAT and returns the NEW recommended difficulty.
        vd.on_share(&worker_key, tmpl.difficulty)
    };
    // Valid share — record count.
    let is_block = meets_difficulty(&hash, tmpl.difficulty);
    {
        let mut round = pool.round.write().await;
        *round.shares.entry(req.miner_address.clone()).or_insert(0) += 1;
        round.total_shares += 1;
    }

    info!("Share accepted from {} worker={} (nonce={}, floor_diff={}, next_vardiff={})",
        req.miner_address, req.worker_name, req.nonce, floor_diff, assigned_share_diff);

    // Persist share to SQLite
    {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default()
            .as_millis() as u64;
        let share_rec = ShareRecord {
            timestamp_ms: now_ms,
            miner_address: req.miner_address.clone(),
            miner_pubkey: req.miner_pubkey.clone(),
            worker_name: req.worker_name.clone(),
            nonce: req.nonce,
            block_index: req.block_index,
            is_block,
        };
        if let Err(e) = pool.db.insert_share(&share_rec).await {
            warn!("Failed to persist share: {}", e);
        }
    }

    // Check if share also meets full block difficulty
    if is_block {
        info!("BLOCK FOUND by {} — nonce={}", req.miner_address, req.nonce);

        let client = reqwest::Client::new();
        let round_start = *pool.round_start_ms.read().await;

        // Build payout transactions BEFORE submitting the block.
        // hash_header() excludes transactions, so embedding them doesn't affect
        // the PoW validity. The block's coinbase runs first in apply_block, so
        // the pool wallet has TESTNET_REWARD available for the payout txs.
        let mut payout_txs = build_payout_txs(&pool, &client, req.block_index, round_start).await;

        // Also include any pending user transactions from the mempool so they
        // get confirmed. Pool payouts come first (they spend the coinbase reward).
        let mempool_txs = fetch_mempool_txs(&client).await;
        if !mempool_txs.is_empty() {
            info!("Including {} mempool tx(s) in block #{}", mempool_txs.len(), req.block_index);
        }
        payout_txs.extend(mempool_txs);

        let mut final_block = candidate.clone();
        final_block.hash = hash;
        final_block.transactions = payout_txs;

        let submitted = submit_block_to_node(&client, &final_block).await;

        if submitted {
            *pool.blocks_found.write().await += 1;
            info!("Block #{} submitted with {} payout txs", req.block_index, final_block.transactions.len());
        }

        return (StatusCode::OK, Json(ShareResponse {
            accepted: true, is_block: true,
            message: format!("Block #{} found and submitted!", req.block_index),
        }));
    }

    (StatusCode::OK, Json(ShareResponse {
        accepted: true, is_block: false,
        message: "Share accepted".into(),
    }))
}

#[derive(Serialize)]
struct MinerEntry {
    address: String,
    shares: u64,
    share_percent: f64,
    shares_24h: u64,
}

#[derive(Serialize)]
struct MinersResponse {
    miners: Vec<MinerEntry>,
    total_shares: u64,
}

async fn get_miners(State(pool): State<Pool>) -> Json<MinersResponse> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let since_24h = now_ms.saturating_sub(24 * 3600 * 1000);

    // Snapshot round state then release lock before async DB calls.
    let (total, addresses): (u64, Vec<(String, u64)>) = {
        let round = pool.round.read().await;
        let addresses = round.shares.iter().map(|(a, &s)| (a.clone(), s)).collect();
        (round.total_shares, addresses)
    };

    let mut miners = Vec::new();
    for (addr, shares) in addresses {
        let shares_24h = pool.db.shares_by_miner_since(&addr, since_24h).await.unwrap_or(0);
        miners.push(MinerEntry {
            address: addr,
            shares,
            share_percent: if total > 0 { shares as f64 / total as f64 * 100.0 } else { 0.0 },
            shares_24h,
        });
    }
    miners.sort_by(|a, b| b.shares.cmp(&a.shares));
    Json(MinersResponse { miners, total_shares: total })
}

// ── Miner stats endpoint ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MinerQuery {
    address: String,
}

#[derive(Serialize)]
struct MinerStatsResponse {
    address: String,
    shares_24h: u64,
    total_paid: u64,
    hashrate: f64,
    last_share_ms: u64,
}

async fn get_miner_stats(
    State(pool): State<Pool>,
    Query(q): Query<MinerQuery>,
) -> Json<MinerStatsResponse> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;

    let since_24h = now_ms.saturating_sub(24 * 3600 * 1000);
    let since_10m = now_ms.saturating_sub(10 * 60 * 1000);

    let shares_24h = pool.db.shares_by_miner_since(&q.address, since_24h).await.unwrap_or(0);
    let total_paid = pool.db.total_paid_to_miner(&q.address).await.unwrap_or(0);

    // Hashrate from last 10-minute window
    let tmpl = pool.template.read().await;
    let share_diff = tmpl.share_difficulty;
    drop(tmpl);

    let recent_shares = pool.db.shares_since(since_10m).await.unwrap_or_default();
    let miner_recent: Vec<_> = recent_shares
        .into_iter()
        .filter(|s| s.miner_address == q.address)
        .collect();
    let hashrate = estimate_hashrate(&miner_recent, share_diff, 10 * 60 * 1000);

    let last_share_ms = pool.db.last_share_ms(&q.address).await.unwrap_or(0);

    Json(MinerStatsResponse {
        address: q.address,
        shares_24h,
        total_paid,
        hashrate,
        last_share_ms,
    })
}

// ── Per-miner hashrate history ────────────────────────────────────────────────

#[derive(Deserialize)]
struct MinerHashrateQuery {
    address: String,
    range: Option<String>,
}

#[derive(Serialize)]
struct MinerHashratePoint {
    timestamp_ms: u64,
    hashrate_hps: f64,
}

#[derive(Serialize)]
struct MinerHashrateResponse {
    points: Vec<MinerHashratePoint>,
}

async fn get_miner_hashrate(
    State(pool): State<Pool>,
    Query(q): Query<MinerHashrateQuery>,
) -> Json<MinerHashrateResponse> {
    let range_ms: u64 = match q.range.as_deref() {
        Some("7d")  => 7 * 86_400_000,
        Some("30d") => 30 * 86_400_000,
        _           => 86_400_000,
    };
    // bucket size: 15min for 24h, 2h for 7d, 8h for 30d
    let bucket_ms: u64 = match q.range.as_deref() {
        Some("7d")  => 2 * 3600 * 1000,
        Some("30d") => 8 * 3600 * 1000,
        _           => 15 * 60 * 1000,
    };

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let since_ms = now_ms.saturating_sub(range_ms);

    let tmpl = pool.template.read().await;
    let share_diff = tmpl.share_difficulty;
    drop(tmpl);

    let hashes_per_share = (1u64 << share_diff) as f64;
    let bucket_s = bucket_ms as f64 / 1000.0;

    let buckets = pool.db
        .shares_by_miner_bucketed(&q.address, since_ms, bucket_ms)
        .await
        .unwrap_or_default();

    let points = buckets
        .into_iter()
        .map(|(bucket_start, count)| MinerHashratePoint {
            timestamp_ms: bucket_start,
            hashrate_hps: count as f64 * hashes_per_share / bucket_s,
        })
        .collect();

    Json(MinerHashrateResponse { points })
}

// ── Workers endpoint ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct WorkerResponse {
    name: String,
    status: String,
    hashrate: f64,
    shares_24h: u64,
    last_seen: u64,
}

#[derive(Serialize)]
struct WorkersResponse {
    workers: Vec<WorkerResponse>,
}

async fn get_miner_workers(
    State(pool): State<Pool>,
    Query(q): Query<MinerQuery>,
) -> Json<WorkersResponse> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;

    const ONLINE_TIMEOUT_MS: u64 = 2 * 60 * 1000; // 2 minutes

    let since_24h = now_ms.saturating_sub(24 * 3600 * 1000);
    let since_2m  = now_ms.saturating_sub(ONLINE_TIMEOUT_MS);

    let tmpl = pool.template.read().await;
    let share_diff = tmpl.share_difficulty;
    drop(tmpl);

    // Get all distinct workers for this miner and their last share timestamp
    let worker_list = pool.db.workers_for_miner(&q.address).await.unwrap_or_default();
    let recent_shares = pool.db.shares_since(since_2m).await.unwrap_or_default();

    let mut workers: Vec<WorkerResponse> = Vec::new();
    for (worker_name, last_share_ms) in worker_list {
        let shares_24h = pool.db.shares_by_worker_since(&q.address, &worker_name, since_24h).await.unwrap_or(0);
        let worker_recent: Vec<_> = recent_shares.iter()
            .filter(|s| s.miner_address == q.address && s.worker_name == worker_name)
            .cloned()
            .collect();
        let hashrate = estimate_hashrate(&worker_recent, share_diff, ONLINE_TIMEOUT_MS);
        let status = if last_share_ms > 0 && now_ms.saturating_sub(last_share_ms) < ONLINE_TIMEOUT_MS {
            "online"
        } else {
            "offline"
        };
        workers.push(WorkerResponse {
            name: worker_name,
            status: status.into(),
            hashrate,
            shares_24h,
            last_seen: last_share_ms,
        });
    }

    // If no workers found yet, return empty list
    Json(WorkersResponse { workers })
}

// ── Miner earnings endpoint ────────────────────────────────────────────────────

#[derive(Serialize)]
struct EarningsPoint {
    date_ms: u64,
    amount_micro: u64,
}

#[derive(Serialize)]
struct EarningsResponse {
    points: Vec<EarningsPoint>,
}

async fn get_miner_earnings(
    State(pool): State<Pool>,
    Query(q): Query<MinerQuery>,
) -> Json<EarningsResponse> {
    let payouts = pool.db.payouts_by_miner(&q.address).await.unwrap_or_default();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let since_30d = now_ms.saturating_sub(30 * 86_400_000);
    let day_ms: u64 = 86_400_000;

    // Group payouts by UTC day bucket
    let mut buckets: HashMap<u64, u64> = HashMap::new();
    for p in payouts.iter().filter(|p| p.timestamp_ms >= since_30d) {
        let bucket = (p.timestamp_ms / day_ms) * day_ms;
        *buckets.entry(bucket).or_insert(0) += p.amount_micro;
    }

    let mut points: Vec<EarningsPoint> = buckets.into_iter()
        .map(|(date_ms, amount_micro)| EarningsPoint { date_ms, amount_micro })
        .collect();
    points.sort_by_key(|p| p.date_ms);

    Json(EarningsResponse { points })
}

// ── Hashrate history endpoint ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct HashrateQuery {
    range: Option<String>,
}

#[derive(Serialize)]
struct HashratePoint {
    timestamp_ms: u64,
    hashrate_hps: f64,
    active_miners: u32,
}

#[derive(Serialize)]
struct HashrateResponse {
    points: Vec<HashratePoint>,
}

async fn get_hashrate(
    State(pool): State<Pool>,
    Query(q): Query<HashrateQuery>,
) -> Json<HashrateResponse> {
    let range_ms: u64 = match q.range.as_deref() {
        Some("7d")  => 7 * 86_400_000,
        Some("30d") => 30 * 86_400_000,
        _           => 86_400_000, // default 24h
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;
    let since = now_ms.saturating_sub(range_ms);

    let points = pool.db.hashrate_snapshots_since(since).await.unwrap_or_default()
        .into_iter()
        .map(|s| HashratePoint {
            timestamp_ms:  s.timestamp_ms,
            hashrate_hps:  s.hashrate_hps,
            active_miners: s.active_miners,
        })
        .collect();

    Json(HashrateResponse { points })
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let wallet = load_or_create_wallet();
    info!("Pool wallet address: {}", wallet.address());
    info!("Pool wallet pubkey:  {}", hex::encode(wallet.public_key()));

    // Fetch initial node status to build first template
    let client = reqwest::Client::new();
    let status = loop {
        match fetch_node_status(&client).await {
            Some(s) => break s,
            None => {
                warn!("Waiting for node at {}…", NODE_URL);
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            }
        }
    };

    let diff = status.difficulty;
    let prev_hash = {
        let bytes = hex::decode(&status.best_hash).unwrap_or_default();
        let mut arr = [0u8; 32];
        let l = bytes.len().min(32);
        arr[..l].copy_from_slice(&bytes[..l]);
        arr
    };

    let initial_template = BlockTemplate {
        index: status.chain_height + 1,
        prev_hash,
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default()
            .as_millis() as u64,
        difficulty: diff,
        share_difficulty: diff.saturating_sub(SHARE_DIFF_OFFSET),
    };

    // Connect to PostgreSQL/TimescaleDB
    let url = db_url();
    let db = Arc::new(Db::connect(&url).await.expect("connect to PostgreSQL/TimescaleDB"));
    info!("Pool database connected: {}", url);

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis() as u64;

    let blocks_found_init = db.count_blocks_found().await.unwrap_or(0);
    info!("Restored blocks_found from DB: {}", blocks_found_init);

    let base_share_diff = diff.saturating_sub(SHARE_DIFF_OFFSET);

    let pool: Pool = Arc::new(PoolState {
        wallet,
        template: RwLock::new(initial_template),
        round: RwLock::new(Round::default()),
        round_start_ms: RwLock::new(now_ms),
        payout_sequence: RwLock::new(0),
        blocks_found: RwLock::new(blocks_found_init),
        vardiff: RwLock::new(VarDiffRegistry::new(base_share_diff)),
        dedup: RwLock::new(TachyonGuard::new()),
        db,
    });

    // Start background watcher
    {
        let pool_clone = pool.clone();
        tokio::spawn(async move { pool_watcher(pool_clone).await });
    }

    // Start hashrate snapshot task
    {
        let pool_clone = pool.clone();
        let db_clone = pool.db.clone();
        tokio::spawn(async move { snapshot_task(pool_clone, db_clone).await });
    }

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_origin([
            "https://pool.taron.network".parse::<HeaderValue>().unwrap(),
            "https://explorer.taron.network".parse::<HeaderValue>().unwrap(),
            "https://wallet.taron.network".parse::<HeaderValue>().unwrap(),
            "https://taron.network".parse::<HeaderValue>().unwrap(),
        ])
        .allow_headers(Any);

    let app = Router::new()
        .route("/pool/status", get(get_pool_status))
        .route("/pool/work",   get(get_work))
        .route("/pool/share",  post(submit_share))
        .route("/pool/miners",   get(get_miners))
        .route("/pool/miner",          get(get_miner_stats))
        .route("/pool/miner-hashrate", get(get_miner_hashrate))
        .route("/pool/miner-workers",  get(get_miner_workers))
        .route("/pool/miner-earnings", get(get_miner_earnings))
        .route("/pool/hashrate",       get(get_hashrate))
        .layer(cors)
        .with_state(pool);

    let addr = format!("127.0.0.1:{}", POOL_PORT);
    info!("Pool server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
