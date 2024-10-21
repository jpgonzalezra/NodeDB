use alloy::primitives::{Address, B256, U256};
use eyre::{eyre, Result};
use reth::api::{NodeTypesWithDB, NodeTypesWithDBAdapter};
use reth::providers::providers::StaticFileProvider;
use reth::providers::StateProviderBox;
use reth::providers::{BlockNumReader, ProviderFactory};
use reth::utils::open_db_read_only;
use reth_chainspec::ChainSpecBuilder;
use reth_db::{mdbx::DatabaseArguments, ClientVersion, DatabaseEnv};
use reth_node_ethereum::EthereumNode;
use revm::primitives::KECCAK_EMPTY;
use revm::state::{Account, AccountInfo, Bytecode};
use revm::{Database, DatabaseCommit, DatabaseRef};
use revm_database::AccountState;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::RwLock;
use log::info;

// Main structure for the Node Database
pub struct NodeDB {
    db_provider: RwLock<StateProviderBox>,
    provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
    accounts: HashMap<Address, NodeDBAccount>,
    db_block: AtomicU64,
    contracts: HashMap<B256, Bytecode>,
}

impl NodeDB {
    // Constructor for NodeDB
    pub fn new(db_path: String) -> Result<Self> {
        // Open the database in read-only mode
        let db_path = Path::new(&db_path);
        let db = Arc::new(open_db_read_only(
            db_path.join("db").as_path(),
            DatabaseArguments::new(ClientVersion::default()),
        )?);

        let spec = Arc::new(ChainSpecBuilder::mainnet().build());
        let factory =
            ProviderFactory::<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>::new(
                db.clone(),
                spec.clone(),
                StaticFileProvider::read_only(db_path.join("static_files"), true)?,
            );

        // Get latest block number and provider
        let db_block = factory.last_block_number()?;
        let db_provider = factory.latest()?;

        Ok(Self {
            db_provider: RwLock::new(db_provider),
            provider_factory: factory,
            accounts: HashMap::new(),
            db_block: AtomicU64::new(db_block),
            contracts: HashMap::new(),
        })
    }

    // Insert account information into the database
    pub fn insert_account_info(
        &mut self,
        account_address: Address,
        account_info: AccountInfo,
        insertion_type: InsertionType,
    ) {
        let mut new_account = NodeDBAccount::new(insertion_type);
        new_account.info = account_info;
        self.accounts.insert(account_address, new_account);
    }

    // Insert storage info into the database.
    pub fn insert_account_storage(
        &mut self,
        account_address: Address,
        slot: U256,
        value: U256,
        insertion_type: InsertionType
    ) -> Result<()> {
        // check to see if we have already created an account for this address
        if let Some(account) = self.accounts.get_mut(&account_address) {
            // there is already an account, just update the storage
            info!("Insert account storage, {} cached {:#?}", account_address, account);
            let slot_value = NodeDBSlot { value, insertion_type: InsertionType::Custom };
            account.storage.insert(slot, slot_value);
            return Ok(());
        }

        // the account does not exist, fetch it from the provider and the insert into the database
        let account = self.basic(account_address)?.unwrap();
        self.insert_account_info(account_address, account, insertion_type);

        // account is now inserted into database, fetch and insert storage
        let node_db_account = self.accounts.get_mut(&account_address).unwrap();
        let slot_value = NodeDBSlot {value, insertion_type: InsertionType::Custom };
        node_db_account.storage.insert(slot, slot_value);
        info!("Insert account storage, {}, after insertion {:#?}", account_address, node_db_account);
        Ok(())
    }

    // Update the provider to access state from the latest block
    fn update_provider(&self) -> Result<()> {
        let current_block = self.provider_factory.last_block_number()?;
        if current_block > self.db_block.load(Ordering::Relaxed) {
            self.db_block.store(current_block, Ordering::Relaxed);
            *self.db_provider.write().unwrap() = self.provider_factory.latest()?;
        }
        Ok(())
    }
}

// Implement the Database trait for NodeDB
impl Database for NodeDB {
    type Error = eyre::Error;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        info!("Basic: Fetching account {}", address);
        // if the account exists and it is custom, we know there is no corresponding on chain state
        if let Some(account) = self.accounts.get(&address) {
            if account.insertion_type == InsertionType::Custom {
                info!("Basic: Got account custom {:#?}", account);
                return Ok(Some(account.info.clone()));
            }
        }

        // fetch the account from the chain
        let account_info = Self::basic_ref(self, address)?.unwrap();
        match self.accounts.get_mut(&address) {
            Some(account) => account.info = {
                info!("Basic: onchain account cached {:#?}", account);
                account_info.clone()
            },
            None => {
                self.insert_account_info(address, account_info.clone(), InsertionType::OnChain);
            }
        }
        info!("Basic: Got account onchain {} {:#?}", address, account_info);

        // this account does not exist and it is a fetched account
        Ok(Some(account_info))
    }

    fn code_by_hash(&mut self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
        panic!("This should not be called, as the code is already loaded");
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        info!("Storage: fetching {}, {}", address, index);
        // custom cache check
        if let Some(account) = self.accounts.get(&address) {
            // check if the slot is in storage
            if let Some(value) = account.storage.get(&index) {
                // the slot is in storage, if it is custom, there is no corresponding onchain state
                // to update it with, just return the value
                if value.insertion_type == InsertionType::Custom {
                    return Ok(value.value);
                } 
                // the account exists and the slot is onchain, continue on so it is fetched and updated
            } 
        }

        // fetch the value and the account
        let value = Self::storage_ref(self, address, index)?;
        match self.accounts.get_mut(&address) {
            Some(account) => {
                // the account exists, just update the slot value
                account.storage.insert(index, NodeDBSlot { value, insertion_type: InsertionType::OnChain });
            }
            None => {
                // the account is onchain and does not exist. insert account via basic and the insert storage value
                let _ = Self::basic(self, address)?;
                let account = self.accounts.get_mut(&address).unwrap();
                account.storage.insert(index, NodeDBSlot { value, insertion_type: InsertionType::OnChain });
            }
        }
        Ok(value)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        Self::block_hash_ref(self, number)
    }
}

// Implement the DatabaseRef trait for NodeDB
impl DatabaseRef for NodeDB {
    type Error = eyre::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        info!("Basic ref : fetching , {}", address);
        if let Some(account) = self.accounts.get(&address) {
            if account.insertion_type == InsertionType::Custom {
                return Ok(Some(account.info.clone()));
            }
        }

        self.update_provider()?;
        let account = self
            .db_provider
            .read()
            .unwrap()
            .basic_account(address)?
            .unwrap();
        let code = self.db_provider.read().unwrap().account_code(address)?;

        let account_info = if let Some(code) = code {
            AccountInfo::new(
                account.balance,
                account.nonce,
                code.hash_slow(),
                Bytecode::new_raw(code.original_bytes()),
            )
        } else {
            AccountInfo::new(
                account.balance,
                account.nonce,
                KECCAK_EMPTY,
                Bytecode::new(),
            )
        };

        Ok(Some(account_info))
    }

    fn code_by_hash_ref(&self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
        panic!("This should not be called, as the code is already loaded");
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        info!("Storage ref: fetching {}, {}", address, index);
        self.update_provider()?;
        let value = self
            .db_provider
            .read()
            .unwrap()
            .storage(address, index.into())?;

        let value = if let Some(storage_val) = value {
            storage_val
        } else {
            U256::ZERO
        };

        Ok(value)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.update_provider()?;
        let blockhash = self.db_provider.read().unwrap().block_hash(number)?;

        if let Some(hash) = blockhash {
            Ok(B256::new(hash.0))
        } else {
            Ok(KECCAK_EMPTY)
        }
    }
}

impl DatabaseCommit for NodeDB {
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        for (address, mut account) in changes {
            if !account.is_touched() {
                continue;
            }
            if account.is_selfdestructed() {
                let db_account = self.accounts.entry(address).or_default();
                db_account.storage.clear();
                db_account.state = AccountState::NotExisting;
                db_account.info = AccountInfo::default();
                continue;
            }
            let is_newly_created = account.is_created();

            if let Some(code) = &mut account.info.code {
                if !code.is_empty() {
                    if account.info.code_hash == KECCAK_EMPTY {
                        account.info.code_hash = code.hash_slow();
                    }
                    self.contracts
                        .entry(account.info.code_hash)
                        .or_insert_with(|| code.clone());
                }
            }

            let db_account = self.accounts.entry(address).or_default();
            db_account.info = account.info;

            db_account.state = if is_newly_created {
                db_account.storage.clear();
                AccountState::StorageCleared
            } else if db_account.state.is_storage_cleared() {
                // Preserve old account state if it already exists
                AccountState::StorageCleared
            } else {
                AccountState::Touched
            };
            db_account.storage.extend(
                account
                    .storage
                    .into_iter()
                    .map(|(key, value)| (key, NodeDBSlot {value: value.present_value(), insertion_type: InsertionType::Custom} )),
            );
        }
    }
}

// Enum representing if an account was fetched from the chain or
// custom data that was inserted into the database
#[derive(Default, Eq, PartialEq, Copy, Clone, Debug)]
pub enum InsertionType {
    Custom,
    #[default]
    OnChain,
}


// Struct representing a storage slot in the database.
// If we insert custom storage, we do not want to fetch 
// from the and overwrite the data. By signaling each slot 
// as custom or onchain, we can accomplish this
#[derive(Default, Eq, PartialEq, Copy, Clone, Debug)]
pub struct NodeDBSlot {
    value: U256,
    insertion_type: InsertionType
}

// Structure to represent an account in the NodeDB
#[derive(Default, Clone, Debug)]
struct NodeDBAccount {
    pub info: AccountInfo,
    pub state: AccountState,
    pub storage: HashMap<U256, NodeDBSlot>,
    pub insertion_type: InsertionType,
}

impl NodeDBAccount {
    pub fn new(insertion_type: InsertionType) -> Self {
        Self {
            info: AccountInfo::default(),
            state: AccountState::NotExisting,
            storage: HashMap::new(),
            insertion_type,
        }
    }
}

/*
#[cfg(test)]
mod nodedb_tests {
    use super::*;
    use alloy::primitives::address;
    use alloy::providers::{Provider, ProviderBuilder, WsConnect};
    use dotenv;
    use futures::StreamExt;

    #[test]
    fn test_fetch_account() {
        let database_path = String::from("/home/dsfreakdude/nodes/base/data");
        let nodedb = NodeDB::new(database_path).unwrap();

        let weth = address!("4200000000000000000000000000000000000006");
        let account = nodedb.basic_ref(weth);
        println!("{:#?}", account);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_in_sync_with_chain() {
        dotenv::dotenv().ok();
        let weth = address!("88A43bbDF9D098eEC7bCEda4e2494615dfD9bB9C");

        // Setup databse connection
        let database_path = std::env::var("DB_PATH").unwrap().parse().unwrap();
        let mut nodedb = NodeDB::new(database_path).unwrap();

        // Setup third party connection for Comparison
        let rpc_url = "http://172.18.0.14:8547";
        let rpc = ProviderBuilder::new().on_http(rpc_url.parse().unwrap());

        // Start a stream of blocks
        let ws_url = WsConnect::new("ws://172.18.0.14:8548");
        let ws = Arc::new(ProviderBuilder::new().on_ws(ws_url).await.unwrap());
        let sub = ws.subscribe_blocks().await.unwrap();
        let mut stream = sub.into_stream();
        while let Some(_) = stream.next().await {
            // fetch reserves from db and rpc
            let db_reserve = nodedb.storage(weth, U256::from(8)).unwrap();
            let rpc_reserve = rpc.get_storage_at(weth, U256::from(8)).await.unwrap();
            println!("DB Reserve {}, RPC Reserve {}", db_reserve, rpc_reserve);
            assert_eq!(db_reserve, rpc_reserve)
        }
    }
}
*/
