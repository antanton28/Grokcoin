use sha2::{Sha256, Digest};
use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use serde::{Serialize, Deserialize};
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Write, Read};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::sync::{Arc, Mutex};
use rand::Rng;
use blake3; // Faster hashing for state

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Transaction {
    sender: String,
    receiver: String,
    amount: u64,
    contract: Option<Vec<u8>>, // Mini-EVM bytecode
    gas: u64, // Gas limit
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Block {
    shard_id: u64,
    prev_hash: String,
    transactions: Vec<Transaction>,
    timestamp: u64,
    nonce: u64,
    hash: String,
    merkle_root: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Shard {
    blocks: Vec<Block>,
    balances: HashMap<String, u64>,
    state: HashMap<String, Vec<u8>>, // Contract storage
}

struct GrokChain {
    shards: Vec<Shard>,
    poh_chain: String,
    peers: Vec<String>,
    shard_threads: Vec<thread::JoinHandle<()>>,
}

impl GrokChain {
    fn new(peers: Vec<String>) -> Self {
        let mut shards = Vec::new();
        for i in 0..640 {
            shards.push(Shard {
                blocks: vec![Self::genesis_block(i)],
                balances: HashMap::new(),
                state: HashMap::new(),
            });
        }
        let poh = Self::init_poh();
        let mut chain = GrokChain { shards, poh_chain: poh, peers, shard_threads: Vec::new() };
        chain.start_shard_threads();
        chain
    }

    fn genesis_block(shard_id: u64) -> Block {
        Block {
            shard_id,
            prev_hash: "0".to_string(),
            transactions: Vec::new(),
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            nonce: 0,
            hash: "00genesis".to_string(),
            merkle_root: "0".to_string(),
        }
    }

    fn init_poh() -> String {
        let mut hasher = Sha256::new();
        hasher.update("GrokChain 10x");
        format!("{:x}", hasher.finalize())
    }

    fn mine_block(&self, shard_id: u64, txs: Vec<Transaction>) -> Block {
        let shard = &self.shards[shard_id as usize];
        let prev_hash = shard.blocks.last().unwrap().hash.clone();
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut nonce = 0;
        let data = bincode::serialize(&txs).unwrap();
        let salt = SaltString::generate(&mut rand::thread_rng());
        let argon2 = Argon2::default();
        let merkle_root = Self::compute_merkle_root(&txs);

        loop {
            let hash_input = format!("{}{}{}", prev_hash, timestamp, nonce);
            let hash = argon2.hash_password(hash_input.as_bytes(), &salt).unwrap();
            let hash_str = hash.to_string();
            if hash_str.starts_with("0") { // Ultra-easy for 1-min blocks
                return Block {
                    shard_id,
                    prev_hash,
                    transactions: txs.clone(),
                    timestamp,
                    nonce,
                    hash: hash_str,
                    merkle_root,
                };
            }
            nonce += 1;
        }
    }

    fn compute_merkle_root(txs: &[Transaction]) -> String {
        let mut hashes: Vec<String> = txs.iter()
            .map(|tx| format!("{:x}", blake3::hash(&bincode::serialize(tx).unwrap())))
            .collect();
        while hashes.len() > 1 {
            let mut new_hashes = Vec::new();
            for chunk in hashes.chunks(2) {
                let combined = if chunk.len() == 2 {
                    chunk[0].clone() + &chunk[1]
                } else {
                    chunk[0].clone()
                };
                new_hashes.push(format!("{:x}", blake3::hash(combined.as_bytes())));
            }
            hashes = new_hashes;
        }
        hashes[0].clone()
    }

    fn update_poh(&mut self, block_hash: &str) {
        let mut hasher = Sha256::new();
        hasher.update(format!("{}{}", self.poh_chain, block_hash));
        self.poh_chain = format!("{:x}", hasher.finalize());
    }

    fn process_tx(&mut self, shard_id: u64, tx: Transaction) -> bool {
        let shard = &mut self.shards[shard_id as usize];
        let sender_bal = shard.balances.entry(tx.sender.clone()).or_insert(10000); // Initial stake
        if *sender_bal < tx.amount || tx.gas > 10000 {
            return false;
        }
        *sender_bal -= tx.amount;
        let receiver_bal = shard.balances.entry(tx.receiver.clone()).or_insert(0);
        *receiver_bal += tx.amount;

        if let Some(code) = tx.contract {
            return self.execute_contract(shard_id, code, tx.amount, tx.gas);
        }
        true
    }

    fn execute_contract(&mut self, shard_id: u64, code: Vec<u8>, value: u64, gas: u64) -> bool {
        let shard = &mut self.shards[shard_id as usize];
        let mut gas_used = 0;
        for op in code { // Simple opcodes
            gas_used += 1;
            if gas_used > gas {
                return false; // Out of gas
            }
            match op {
                0x01 => shard.balances.entry("Contract".to_string()).and_modify(|b| *b += value), // Add value
                0x02 => shard.state.insert("data".to_string(), vec![value as u8]), // Store
                _ => continue, // Ignore invalid
            }
        }
        gas_used <= gas
    }

    fn save_state(&self) {
        let data = bincode::serialize(&self.shards).unwrap();
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open("grokchain_10x_state.bin")
            .unwrap();
        file.write_all(&data).unwrap();
    }

    fn load_state(&mut self) {
        if let Ok(mut file) = File::open("grokchain_10x_state.bin") {
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            self.shards = bincode::deserialize(&buffer).unwrap_or_else(|_| self.shards.clone());
        }
    }

    fn broadcast_block(&self, block: Block) {
        for peer in &self.peers {
            if let Ok(mut stream) = TcpStream::connect(peer) {
                let data = bincode::serialize(&block).unwrap();
                stream.write_all(&data).unwrap();
            }
        }
    }

    fn start_shard_threads(&mut self) {
        let chain = Arc::new(Mutex::new(self.shards.clone()));
        for shard_id in 0..640 {
            let chain = chain.clone();
            let handle = thread::spawn(move || {
                let mut local_chain = chain.lock().unwrap();
                let txs = vec![Transaction {
                    sender: "User".to_string(),
                    receiver: "Other".to_string(),
                    amount: 1,
                    contract: None,
                    gas: 100,
                }];
                let block = GrokChain::mine_block(&local_chain, shard_id, txs);
                local_chain[shard_id as usize].blocks.push(block);
            });
            self.shard_threads.push(handle);
        }
    }
}

fn run_p2p_server(chain: Arc<Mutex<GrokChain>>) {
    let listener = TcpListener::bind("127.0.0.1:9000").unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut buffer = Vec::new();
        stream.read_to_end(&mut buffer).unwrap();
        let block: Block = bincode::deserialize(&buffer).unwrap();
        let mut chain = chain.lock().unwrap();
        chain.shards[block.shard_id as usize].blocks.push(block);
    }
}

fn main() {
    let peers = vec!["127.0.0.1:9001".to_string()];
    let chain = Arc::new(Mutex::new(GrokChain::new(peers)));
    let chain_clone = chain.clone();

    // Start P2P server
    thread::spawn(move || run_p2p_server(chain_clone));

    let mut chain = chain.lock().unwrap();
    chain.load_state();
    println!("GrokChain 10x launched - PoH: {}", chain.poh_chain);

    // Test: Mine and process txs
    let txs = vec![
        Transaction {
            sender: "Alice".to_string(),
            receiver: "Bob".to_string(),
            amount: 100,
            contract: None,
            gas: 100,
        },
        Transaction {
            sender: "Bob".to_string(),
            receiver: "Charlie".to_string(),
            amount: 50,
            contract: Some(vec![0x01, 0x02]), // Contract deposit + store
            gas: 200,
        },
    ];

    println!("Mining block in shard 0...");
    let start = SystemTime::now();
    let block = chain.mine_block(0, txs.clone());
    chain.shards[0].blocks.push(block.clone());
    chain.broadcast_block(block);
    for tx in txs {
        chain.process_tx(0, tx);
    }
    let elapsed = start.elapsed().unwrap().as_secs_f64();
    println!("Mined block: {:?}", chain.shards[0].blocks.last());
    println!("Time: {:.2}s", elapsed);

    // Simulate 1M TPS test
    let tx_per_shard = 1562; // 1M / 640
    let mut total_txs = 0;
    let start = SystemTime::now();
    for shard_id in 0..640 {
        for _ in 0..tx_per_shard {
            let tx = Transaction {
                sender: "User".to_string(),
                receiver: "Other".to_string(),
                amount: 1,
                contract: None,
                gas: 100,
            };
            if chain.process_tx(shard_id, tx) {
                total_txs += 1;
            }
        }
    }
    let elapsed = start.elapsed().unwrap().as_secs_f64();
    let tps = total_txs as f64 / elapsed;
    println!("Processed {} txs in {:.2}s - TPS: {:.0}", total_txs, elapsed, tps);

    // Check state
    let shard = &chain.shards[0];
    println!("Balances: Alice: {}, Bob: {}, Charlie: {}, Contract: {}", 
             shard.balances.get("Alice").unwrap_or(&0),
             shard.balances.get("Bob").unwrap_or(&0),
             shard.balances.get("Charlie").unwrap_or(&0),
             shard.balances.get("Contract").unwrap_or(&0));
    println!("Contract storage: {:?}", shard.state.get("data"));

    chain.save_state();
}
