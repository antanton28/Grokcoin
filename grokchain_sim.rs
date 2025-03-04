use blake3;
use ed25519_dalek::{Keypair, Signer, Verifier, PublicKey, Signature};
use serde::{Serialize, Deserialize};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use rocksdb::{DB, Options};
use rand::rngs::OsRng;
use std::thread;
use sha2::{Sha256, Digest};

const MAX_TXS_PER_BLOCK: usize = 1000;
const BAN_DURATION: u64 = 30 * 60; // 30 minute
const RATE_LIMIT: usize = 5; // Max 5 blocuri/secundă/IP
const BLOCK_REWARD: u64 = 10; // 10 GROK/block
const TX_FEE_PERCENT: f64 = 0.01; // 1% taxe

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Transaction {
    sender: String,
    receiver: String,
    amount: u64,
    id: u64,
    signature: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Block {
    prev_hash: String,
    transactions: Vec<Transaction>,
    timestamp: u64,
    nonce: u64,
    hash: String,
    difficulty: u32,
    miner_ip: String,
    poh_index: usize,
    fees: u64, // Taxe colectate
}

struct GrokChainSim {
    db: Arc<DB>,
    balances: Arc<Mutex<HashMap<String, u64>>>, // Solduri tranzacții
    grok_balances: Arc<Mutex<HashMap<String, u64>>>, // Solduri GROK
    keypair: Keypair,
    poh_chain: Arc<Mutex<Vec<String>>>,
    blacklist: Arc<Mutex<HashMap<String, u64>>>,
    failed_attempts: Arc<Mutex<HashMap<String, usize>>>,
    rate_limiter: Arc<Mutex<HashMap<String, Vec<u64>>>>,
    peers: Vec<String>,
}

impl GrokChainSim {
    fn new(peers: Vec<String>) -> Self {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = Arc::new(DB::open(&opts, "grokchain_db").unwrap());
        let poh_chain = vec![format!("{:x}", Sha256::digest(b"GrokChain PoH Seed"))];
        let genesis = Block {
            prev_hash: "0".to_string(),
            transactions: vec![],
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            nonce: 0,
            hash: "00genesis".to_string(),
            difficulty: 4,
            miner_ip: "127.0.0.1".to_string(),
            poh_index: 0,
            fees: 0,
        };
        db.put(b"block_0", &bincode::serialize(&genesis).unwrap()).unwrap();

        GrokChainSim {
            db,
            balances: Arc::new(Mutex::new(HashMap::new())),
            grok_balances: Arc::new(Mutex::new(HashMap::new())),
            keypair: Keypair::generate(&mut OsRng),
            poh_chain: Arc::new(Mutex::new(poh_chain)),
            blacklist: Arc::new(Mutex::new(HashMap::new())),
            failed_attempts: Arc::new(Mutex::new(HashMap::new())),
            rate_limiter: Arc::new(Mutex::new(HashMap::new())),
            peers,
        }
    }

    fn get_block_count(&self) -> usize {
        let mut count = 0;
        while self.db.get(format!("block_{}", count).as_bytes()).unwrap().is_some() {
            count += 1;
        }
        count
    }
}

fn main() {
    let peers = vec!["127.0.0.1:9001".to_string()];
    let chain = Arc::new(GrokChainSim::new(peers));
    let block_count = chain.get_block_count();
    println!("Initial chain length: {}", block_count);
}
