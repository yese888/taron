<div align="center">

```
████████╗ █████╗ ██████╗  ██████╗ ███╗   ██╗
╚══██╔══╝██╔══██╗██╔══██╗██╔═══██╗████╗  ██║
   ██║   ███████║██████╔╝██║   ██║██╔██╗ ██║
   ██║   ██╔══██║██╔══██╗██║   ██║██║╚██╗██║
   ██║   ██║  ██║██║  ██║╚██████╔╝██║ ╚████║
   ╚═╝   ╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝
```

**TARON — CPU-Only Cryptocurrency · Instant Finality**
*Proof of Sequential Chain (PoSC) · SEQUAL-256 · Written from scratch in Rust*

`TAR` · 1,000,000,000 max supply · Linux & Windows · Single binary · No ASIC · No GPU

---

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Language: Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status: Testnet](https://img.shields.io/badge/Status-Testnet-yellow.svg)]()
[![Platform: Linux & Windows](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows-green.svg)]()
[![Explorer](https://img.shields.io/badge/Explorer-explorer.taron.network-blue.svg)](https://explorer.taron.network)

</div>

---

## Abstract

TARON is a CPU-only cryptocurrency with **sub-200ms transaction finality**. Its core innovation — the **Proof of Sequential Chain (PoSC)** — embeds a proof-of-work directly into each transaction. Unlike any existing PoW coin, transactions do not wait for blocks: they are final the moment they propagate through the network (~150ms total). SEQUAL-256, a custom sequential hash function, makes GPU and ASIC acceleration mathematically impossible. The entire stack — node, miner, wallet — ships as a single Rust binary with no external dependencies.

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Problem Statement](#2-problem-statement)
3. [Architecture — Proof of Sequential Chain](#3-architecture--proof-of-sequential-chain)
4. [SEQUAL-256 — Custom CPU Hash Function](#4-sequal-256--custom-cpu-hash-function)
5. [Transaction Model](#5-transaction-model)
6. [Network Protocol](#6-network-protocol)
7. [Supply and Distribution](#7-supply-and-distribution)
8. [Comparison with Existing Systems](#8-comparison-with-existing-systems)
9. [CLI Reference](#9-cli-reference)
10. [Technical Specifications](#10-technical-specifications)

---

## 1. Introduction

Bitcoin takes 60 minutes to finalize. Ethereum takes 15 seconds. Kaspa, the fastest PoW coin today, takes ~10 seconds. NANO achieves near-instant finality but has no mining and was fully pre-distributed via faucet in 2014.

**TARON achieves sub-200ms finality on a PoSC coin.** This is not an engineering optimization — it is a fundamental architectural difference. There are no blocks to wait for. Each transaction carries its own proof of work, computed by the sender in ~100ms. Nodes validate and finalize instantly.

TARON is the first proof-of-work cryptocurrency where transactions confirm faster than a credit card tap.

---

## 2. Problem Statement

### 2.1 Finality Comparison

| System | Architecture | Avg. Finality | Mining | ASIC-Resistant |
|--------|-------------|---------------|--------|----------------|
| Bitcoin | Linear chain (PoW) | ~60 min | SHA-256 | ❌ |
| Ethereum | Linear chain (PoS) | ~15 sec | Stake | N/A |
| Kaspa | DAG / GhostDAG | ~10 sec | kHeavyHash | ❌ |
| NANO | Block-lattice (dPoS) | ~0.2 sec | None (faucet) | N/A |
| Monero | Linear chain (PoW) | ~2 min | RandomX | ✅ |
| **TARON** | **Blockless PoSC** | **<0.2 sec** | **SEQUAL-256** | **✅** |

### 2.2 The Mining Inequality Problem

GPU and ASIC dominance leads to mining centralization. RandomX uses memory-hardness to deter ASICs. TARON uses **sequential dependency**: each SEQUAL-256 step depends on the result of the previous step. No amount of parallelism accelerates it. A machine with 10,000 GPU cores computes SEQUAL-256 at the same speed as a single CPU core.

---

## 3. Architecture — Proof of Sequential Chain

### 3.1 Core Principle

In TARON, **every transaction is its own block**. To send TAR, the sender's node computes a PoSC proof for that transaction in ~100ms. Any receiving node verifies the proof in microseconds. Once verified, the transaction is immediately broadcast and considered final.

$$T_{\text{final}} = T_{\text{PoSC}} + T_{\text{prop}} \approx 100\text{ms} + 50\text{ms} = 150\text{ms}$$

### 3.2 Sequential Chain Property

Each hash step is mathematically bound to its predecessor:

$$H_i = f(H_{i-1}, \phi) \quad \forall\, i \in [0, N)$$

Step $H_i$ cannot be computed before $H_{i-1}$ completes. This is the GPU/ASIC resistance mechanism.

### 3.3 Coinbase Mining

Dedicated miners search for block hashes meeting a difficulty target (leading zero bits in SEQUAL-256 output). The difficulty auto-adjusts every 10 blocks targeting a 30-second block time. Mining rewards new TAR into circulation. Both PoSC transactions and coinbase mining coexist — mining distributes new TAR while PoSC provides instant finality.

### 3.4 Double-Spend Prevention

1. **Sequence numbers** — each transaction must reference the account's current sequence number.
2. **PoSC chaining** — the proof input includes the previous transaction hash. Forking requires recomputing the entire chain from the fork point (~100ms per transaction).

---

## 4. SEQUAL-256 — Custom CPU Hash Function

### 4.1 Design Goals

1. **Sequential dependency** — step N cannot begin before step N-1 completes
2. **Memory-access pattern** — pseudo-random reads from a 4MB scratchpad (fits in L3 cache, incompatible with GPU shared memory)
3. **Integer-heavy** — 64-bit MUL, ADD, XOR, ROTATE (CPU-native)
4. **Non-invertible** — no known algebraic shortcut
5. **256-bit output** — SHA3-256 compatible

### 4.2 GPU/ASIC Resistance

| Optimization Vector | GPU Capability | SEQUAL-256 Response |
|--------------------|----------------|---------------------|
| Parallelism | Thousands of cores | Sequential dependency eliminates parallel speedup |
| Memory bandwidth | High BW, small L1 per-core | 4MB scratchpad exceeds GPU L1/L2 per-core |
| Custom silicon (ASIC) | Fixed pipeline | Sequential ops + scratchpad make ASIC uneconomical |
| FPGA | Configurable | Sequential dependency limits pipelining depth |

---

## 5. Transaction Model

### 5.1 Transaction Structure

```
version       u8        Protocol version
sender        [u8;32]   Ed25519 public key
recipient     [u8;32]   Ed25519 public key
amount        u64       In µTAR (1 TAR = 1,000,000 µTAR)
fee           u64       In µTAR (minimum 1 µTAR, burned)
sequence      u64       Monotonically increasing per account
timestamp_ms  u64       Unix milliseconds
posc_proof    [u8;32]   SEQUAL-256 proof
posc_steps    u32       Steps computed (~100ms worth)
signature     [u8;64]   Ed25519 signature over all fields
```

Total size: ~256 bytes fixed.

### 5.2 Address Format

```
tar1 + hex(pubkey)
```

68 characters total. The public key is directly embedded in the address, making it fully reversible.

Example: `tar11d391588bed44f12c103c3eeced531ecd2e8564b4a2d510d7691bec1d7321eaf`

---

## 6. Network Protocol

### 6.1 P2P Architecture

Custom TCP gossip protocol. Three discovery mechanisms:

1. **Explicit seed** — `--seed <ip:port>`
2. **DNS seed** — `seed.taron.network:8333` (testnet)
3. **UDP local broadcast** — port 8334 (LAN fallback)

### 6.2 Message Types

| Message | Purpose |
|---------|---------|
| `Hello` | Handshake — version, chain head |
| `Ping / Pong` | Keepalive |
| `GetPeers / Peers` | Exchange known peers |
| `Tx` | Broadcast transaction |
| `TxAck` | Acknowledge validated transaction |
| `NewBlock` | Broadcast mined block |
| `GetBlocks / Blocks` | Initial Block Download (IBD) |
| `GetChainHeight / ChainHeight` | Sync handshake |

### 6.3 Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 8333 | TCP | P2P communication |
| 8334 | UDP | Local discovery broadcast |
| 8082 | TCP | HTTP REST API (optional, `--rpc-port`) |

### 6.4 REST API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/status` | Height, difficulty, peers, supply |
| GET | `/api/v1/blocks?offset=0&limit=20` | Paginated blocks |
| GET | `/api/v1/blocks/:index` | Block by height |
| GET | `/api/v1/tx/:hash` | Transaction lookup |
| GET | `/api/v1/mempool` | Pending transactions |
| GET | `/api/v1/accounts` | Accounts sorted by balance |
| GET | `/api/v1/accounts/:address` | Account by `tar1...` address |
| GET | `/api/v1/accounts/:address/blocks` | Blocks mined by address |
| GET | `/api/v1/peers` | Connected peers |
| POST | `/api/v1/submit_tx` | Submit signed transaction |
| POST | `/api/v1/submit_block` | Submit mined block (pool) |

---

## 7. Supply and Distribution

**1,000,000,000 TAR** — 2% development premine (20,000,000 TAR), no ICO, no VC allocation. 98% distributed through CPU mining.

| Year | Reward/block | New TAR/year | Cumulative |
|------|-------------|--------------|------------|
| 1 | 15.85 TAR | ~500M | ~500M |
| 2 | 7.93 TAR | ~250M | ~750M |
| 3 | 3.96 TAR | ~125M | ~875M |
| 4 | 1.98 TAR | ~62.5M | ~937M |
| 5 | 0.99 TAR | ~31.3M | ~968M |
| … | … | … | → 1B |

Block time target: **30 seconds** (DAA adjusts every 10 blocks).

---

## 8. Comparison with Existing Systems

| Feature | Bitcoin | Kaspa | NANO | Monero | **TARON** |
|---------|---------|-------|------|--------|-----------|
| Finality | 60min | ~10sec | ~0.2sec | 2min | **<0.2sec** |
| PoW mining | ✅ | ✅ | ❌ | ✅ | **✅** |
| CPU-only | ❌ | ❌ | N/A | ✅ | **✅** |
| Low premine | ❌ (0%) | ❌ (0%) | ❌ (faucet) | ❌ (0%) | **2% dev** |
| Instant finality | ❌ | ❌ | ✅ | ❌ | **✅** |
| ASIC-resistant | ❌ | ❌ | N/A | ✅ | **✅** |
| Spam-resistant | ✅ | ✅ | ❌ | ✅ | **✅** |
| Single binary | ❌ | ❌ | ❌ | ❌ | **✅** |

---

## 9. CLI Reference

```bash
# Start node (no mining)
taron [--testnet] node start [--port 8333] [--rpc-port 8082]

# Solo mining
taron [--testnet] node start --mine --threads 4

# Pool mining (with CLI wallet — auto-detected)
taron [--testnet] node start \
  --pool https://pool-api.taron.network \
  --threads 4 --worker my-rig

# Pool mining (with web wallet address)
taron [--testnet] node start \
  --pool https://pool-api.taron.network \
  --threads 4 --worker my-rig \
  --address tar1YOUR_ADDRESS_HERE

# Wallet
taron [--testnet] wallet generate
taron [--testnet] wallet info [--key /path/to/wallet.key]

# Send TAR
taron [--testnet] send <tar1address> <amount>

# Node status
taron [--testnet] status

# Benchmarks
taron [--testnet] bench [--count N]
```

### Connect to testnet

```bash
# Seed node resolves automatically via seed.taron.network
taron --testnet node start --mine --threads 4 --rpc-port 8082
```

---

## 10. Technical Specifications

| Parameter | Value |
|-----------|-------|
| Language | Rust (edition 2021) |
| Platforms | Linux x86_64, Windows x64 |
| Wallet | Ed25519 (dalek) |
| Hash function | SEQUAL-256 (custom) + SHA3-256 |
| Block time target | 30 seconds |
| DAA window | Every 10 blocks |
| Max supply | 1,000,000,000 TAR |
| Micro-unit | 1 µTAR = 0.000001 TAR |
| Initial block reward | 15.85 TAR |
| Halving interval | Annual |
| Address prefix | `tar1` |
| P2P port | 8333 (TCP) |
| Discovery port | 8334 (UDP) |
| RPC port | 8082 (optional HTTP) |
| Persistence | RocksDB (chain) + bincode (ledger) |
| Data directory | `~/.taron-testnet/` (testnet) · `~/.taron/` (mainnet) |

---

## Troubleshooting

### Block sync rejected — `prev_hash mismatch`

If you see this warning when starting your node:
```
[SYNC] Block #N rejected: prev_hash mismatch
```

Your local chain data is out of sync with the network. This can happen if you ran the node during an earlier testnet phase.

**Fix:** delete your local chain data and restart — the node will automatically re-download the full chain from the seed node:

```bash
rm -rf ~/.taron-testnet/chain.db ~/.taron-testnet/ledger.bin
./target/release/taron --testnet node start --pool https://pool-api.taron.network --threads 4
```

> Your wallet and address are stored separately in `~/.taron-testnet/wallet.key` and are **not affected** by this operation.

---

## License

MIT — see [LICENSE](LICENSE)

---

<div align="center">

*The only PoSC coin where your CPU confirms a payment faster than your credit card.*

[taron.network](https://taron.network) · [explorer.taron.network](https://explorer.taron.network) · [pool.taron.network](https://pool.taron.network) · [wallet.taron.network](https://wallet.taron.network)

</div>
