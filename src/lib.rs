use eyre::Result;
use parking_lot::RwLock;
use reth_chainspec::ChainSpecBuilder;
use reth_db::{mdbx::DatabaseArguments, open_db_read_only, ClientVersion, DatabaseEnv};
use reth_node_api::NodeTypesWithDBAdapter;
use reth_node_ethereum::EthereumNode;
use reth_provider::{
    providers::StaticFileProvider, BlockNumReader, ProviderFactory, StateProviderBox,
};
use revm::context::DBErrorMarker;
use revm::database::AccountState;
use revm::primitives::{Address, B256, KECCAK_EMPTY, U256};
use revm::state::{Account, AccountInfo, Bytecode};
use revm::{Database, DatabaseCommit, DatabaseRef};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug)]
pub struct NodeDBError(pub String);

impl fmt::Display for NodeDBError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeDBError: {}", self.0)
    }
}

impl Error for NodeDBError {}

impl DBErrorMarker for NodeDBError {}

// Main structure for the Node Database
pub struct NodeDB {
    db_provider: RwLock<StateProviderBox>,
    provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
    accounts: HashMap<Address, NodeDBAccount>,
    db_block: AtomicU64,
    contracts: HashMap<B256, Bytecode>,
    accessed_slots: RwLock<HashMap<Address, HashSet<U256>>>,
    tracing_enabled: bool,
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

        // Create a ProviderFactory
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
            accessed_slots: RwLock::new(HashMap::new()),
            tracing_enabled: false,
        })
    }

    pub fn enable_tracing(&mut self) -> Result<()> {
        let mut slots = self.accessed_slots.write();

        self.tracing_enabled = true;
        slots.clear();
        Ok(())
    }

    pub fn disable_tracing(&mut self) {
        self.tracing_enabled = false;
    }

    pub fn get_accessed_slots(&self, target_address: Address) -> Result<Vec<U256>> {
        let slots = self.accessed_slots.read();
        Ok(slots
            .get(&target_address)
            .map(|slot_set| slot_set.iter().cloned().collect())
            .unwrap_or_default())
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
        insertion_type: InsertionType,
    ) -> Result<()> {
        // If this account already exists, just update the storage slot
        if let Some(account) = self.accounts.get_mut(&account_address) {
            // slot value is marked as custom since this is a custom insertion
            let slot_value = NodeDBSlot {
                value,
                insertion_type: InsertionType::Custom,
            };
            account.storage.insert(slot, slot_value);
            return Ok(());
        }

        // The account does not exist. Fetch account information from provider and insert account
        // into database
        let account = self.basic(account_address)?.unwrap();
        self.insert_account_info(account_address, account, insertion_type);

        // The account is now in the database, so fetch and insert the storage value
        let node_db_account = self.accounts.get_mut(&account_address).unwrap();
        let slot_value = NodeDBSlot {
            value,
            insertion_type: InsertionType::Custom,
        };
        node_db_account.storage.insert(slot, slot_value);

        Ok(())
    }

    // Update the provider to access state from the latest block
    fn update_provider(&self) -> Result<()> {
        if let Ok(current_block) = self.provider_factory.last_block_number() {
            if current_block > self.db_block.load(Ordering::Relaxed) {
                self.db_block.store(current_block, Ordering::Relaxed);
                *self.db_provider.write() = self.provider_factory.latest()?;
            }
        }
        Ok(())
    }
}

// Implement the Database trait for NodeDB
impl Database for NodeDB {
    type Error = NodeDBError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        // If the account exists and it is custom, we know there is no corresponding on chain state
        if let Some(account) = self.accounts.get(&address) {
            if account.insertion_type == InsertionType::Custom {
                return Ok(Some(account.info.clone()));
            }
        }

        // Fetch the account from the chain
        let account_info_opt = Self::basic_ref(self, address)?;

        let account_info = match account_info_opt {
            Some(info) => info,
            None => return Ok(None),
        };

        match self.accounts.get_mut(&address) {
            Some(account) => account.info = account_info.clone(),
            None => self.insert_account_info(address, account_info.clone(), InsertionType::OnChain),
        }

        Ok(Some(account_info))
    }

    fn code_by_hash(&mut self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
        panic!("This should not be called, as the code is already loaded");
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        // Log slot access if tracing is enabled
        if self.tracing_enabled {
            let mut slots = self.accessed_slots.write();
            slots.entry(address).or_default().insert(index);
        }
        // Check if the account and the slot exist
        if let Some(account) = self.accounts.get(&address) {
            if let Some(value) = account.storage.get(&index) {
                // The slot is in storage. If it is custom, there is no corresponding onchain state
                // to update it with, just return the value
                if value.insertion_type == InsertionType::Custom {
                    return Ok(value.value);
                }
                // The account exists and the slot is onchain, continue on so it is fetched and updated
            }
        }

        // Fetch the storage value
        let value = Self::storage_ref(self, address, index)?;

        // If the account exists, just update the storage. Otherwise, fetch and create a new
        // account before inserting the storage value
        match self.accounts.get_mut(&address) {
            Some(account) => {
                account.storage.insert(
                    index,
                    NodeDBSlot {
                        value,
                        insertion_type: InsertionType::OnChain,
                    },
                );
            }
            None => {
                let _ = Self::basic(self, address)?;
                let account = self.accounts.get_mut(&address).unwrap();
                account.storage.insert(
                    index,
                    NodeDBSlot {
                        value,
                        insertion_type: InsertionType::OnChain,
                    },
                );
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
    type Error = NodeDBError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        // If the account exists and it is custom, just return it. Otherwise update from the chain
        if let Some(account) = self.accounts.get(&address) {
            if account.insertion_type == InsertionType::Custom {
                return Ok(Some(account.info.clone()));
            }
        }

        // Fetch info
        self.update_provider()
            .map_err(|e| NodeDBError(e.to_string()))?;

        let db_provider = self.db_provider.read();

        let account_opt = db_provider
            .basic_account(&address)
            .map_err(|e| NodeDBError(e.to_string()))?;

        let account = match account_opt {
            Some(acc) => acc,
            None => return Ok(None),
        };

        let code = db_provider
            .account_code(&address)
            .map_err(|e| NodeDBError(e.to_string()))?;

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
        // Log slot access if tracing is enabled
        if self.tracing_enabled {
            let mut slots = self.accessed_slots.write();
            slots.entry(address).or_default().insert(index);
        }
        // if account exista and slot is custom, just return value
        if let Some(account) = self.accounts.get(&address) {
            if let Some(value) = account.storage.get(&index) {
                if value.insertion_type == InsertionType::Custom {
                    return Ok(value.value);
                }
            }
        }

        self.update_provider()
            .map_err(|e| NodeDBError(e.to_string()))?;

        let value = self
            .db_provider
            .read()
            .storage(address, index.into())
            .map_err(|e| NodeDBError(e.to_string()))?;

        let value = if let Some(storage_val) = value {
            storage_val
        } else {
            U256::ZERO
        };

        Ok(value)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.update_provider()
            .map_err(|e| NodeDBError(e.to_string()))?;

        let blockhash = self
            .db_provider
            .read()
            .block_hash(number)
            .map_err(|e| NodeDBError(e.to_string()))?;

        if let Some(hash) = blockhash {
            Ok(B256::new(hash.0))
        } else {
            Ok(KECCAK_EMPTY)
        }
    }
}

impl DatabaseCommit for NodeDB {
    fn commit(&mut self, changes: HashMap<Address, Account, foldhash::fast::RandomState>) {
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
            db_account
                .storage
                .extend(account.storage.into_iter().map(|(key, value)| {
                    (
                        key,
                        NodeDBSlot {
                            value: value.present_value(),
                            insertion_type: InsertionType::Custom,
                        },
                    )
                }));
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
// from the chain and overwrite the data. By signaling each slot
// as custom or onchain, we can accomplish this
#[derive(Default, Eq, PartialEq, Copy, Clone, Debug)]
pub struct NodeDBSlot {
    value: U256,
    insertion_type: InsertionType,
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
