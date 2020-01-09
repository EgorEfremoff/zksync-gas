use crate::eth_tx_helpers::get_ethereum_transaction;
use crate::events_state::EventsState;
use crate::genesis_state::get_genesis_account;
use crate::rollup_ops::RollupOpsBlock;
use crate::storage_interactor;
use crate::tree_state::TreeState;
use ethabi;
use failure::format_err;
use models::node::{AccountMap, AccountUpdate};
use std::str::FromStr;
use storage::ConnectionPool;
use web3::contract::Contract;
use web3::types::{H160, H256};
use web3::{Transport, Web3};

/// Storage state update:
/// - None - The state is updated completely last time - start from fetching the new events
/// - Events - The events fetched and saved successfully - now get operations from them and update tree
/// - Operations - There are operations that are not presented in the tree state - update tree state
#[derive(Debug)]
pub enum StorageUpdateState {
    None,
    Events,
    Operations,
}

/// Description of data restore driver
pub struct DataRestoreDriver<T: Transport> {
    /// Database connection pool
    pub connection_pool: ConnectionPool,
    /// Web3 provider endpoint
    pub web3: Web3<T>,
    /// Provides Ethereum Governance contract unterface
    pub governance_contract: (ethabi::Contract, Contract<T>),
    /// Provides Ethereum Rollup contract unterface
    pub franklin_contract: (ethabi::Contract, Contract<T>),
    /// Flag that indicates that state updates are running
    pub run_update: bool,
    /// Rollup contract events state
    pub events_state: EventsState,
    /// Rollup accounts state
    pub tree_state: TreeState,
    /// The step distance of viewing events in the ethereum blocks
    pub eth_blocks_step: u64,
    /// The distance to the last ethereum block
    pub end_eth_blocks_offset: u64,
}

impl<T: Transport> DataRestoreDriver<T> {
    /// Returns new data restore driver with empty events and tree states
    ///
    /// # Arguments
    ///
    /// * `connection_pool` - Database connection pool
    /// * `web3_transport` - Web3 provider transport
    /// * `governance_contract_eth_addr` - Governance contract address
    /// * `franklin_contract_eth_addr` - Rollup contract address
    /// * `eth_blocks_step` - The step distance of viewing events in the ethereum blocks
    /// * `end_eth_blocks_offset` - The distance to the last ethereum block
    ///
    pub fn new_empty(
        connection_pool: ConnectionPool,
        web3_transport: T,
        governance_contract_eth_addr: H160,
        franklin_contract_eth_addr: H160,
        eth_blocks_step: u64,
        end_eth_blocks_offset: u64,
    ) -> Result<Self, failure::Error> {
        let web3 = Web3::new(web3_transport);

        let governance_contract = {
            let abi_string = serde_json::Value::from_str(models::abi::GOVERNANCE_CONTRACT)
                .map_err(|e| format_err!("No governance contract abi: {}", e.to_string()))?
                .get("abi")
                .ok_or_else(|| format_err!("No governance contract abi"))?
                .to_string();
            let abi = ethabi::Contract::load(abi_string.as_bytes())
                .map_err(|e| format_err!("No governance contract abi: {}", e.to_string()))?;
            (
                abi.clone(),
                Contract::new(web3.eth(), governance_contract_eth_addr, abi.clone()),
            )
        };

        let franklin_contract = {
            let abi_string = serde_json::Value::from_str(models::abi::FRANKLIN_CONTRACT)
                .map_err(|e| format_err!("No franklin contract abi: {}", e.to_string()))?
                .get("abi")
                .ok_or_else(|| format_err!("No franklin contract abi"))?
                .to_string();
            let abi = ethabi::Contract::load(abi_string.as_bytes())
                .map_err(|e| format_err!("No franklin contract abi: {}", e.to_string()))?;
            (
                abi.clone(),
                Contract::new(web3.eth(), franklin_contract_eth_addr, abi.clone()),
            )
        };

        let events_state = EventsState::new();

        let tree_state = TreeState::new();

        Ok(Self {
            connection_pool,
            web3,
            governance_contract,
            franklin_contract,
            run_update: false,
            events_state,
            tree_state,
            eth_blocks_step,
            end_eth_blocks_offset,
        })
    }

    /// Returns the new data restore driver state with 'genesis' state - tree with inserted genesis account
    ///
    /// # Arguments
    ///
    /// * `connection_pool` - Database connection pool
    /// * `web3_transport` - Web3 provider transport
    /// * `governance_contract_eth_addr` - Governance contract address
    /// * `governance_contract_genesis_tx_hash` - Governance contract creation tx hash
    /// * `franklin_contract_eth_addr` - Rollup contract address
    /// * `franklin_contract_genesis_tx_hash` - Rollup contract creation tx hash
    /// * `eth_blocks_step` - The step distance of viewing events in the ethereum blocks
    /// * `end_eth_blocks_offset` - The distance to the last ethereum block
    ///
    pub fn new_with_genesis_acc(
        connection_pool: ConnectionPool,
        web3_transport: T,
        governance_contract_eth_addr: H160,
        governance_contract_genesis_tx_hash: H256,
        franklin_contract_eth_addr: H160,
        franklin_contract_genesis_tx_hash: H256,
        eth_blocks_step: u64,
        end_eth_blocks_offset: u64,
    ) -> Result<Self, failure::Error> {
        let web3 = Web3::new(web3_transport);

        let governance_contract = {
            let abi_string = serde_json::Value::from_str(models::abi::GOVERNANCE_CONTRACT)
                .map_err(|e| format_err!("No governance contract abi: {}", e.to_string()))?
                .get("abi")
                .ok_or_else(|| format_err!("No governance contract abi"))?
                .to_string();
            let abi = ethabi::Contract::load(abi_string.as_bytes())
                .map_err(|e| format_err!("No governance contract abi: {}", e.to_string()))?;
            (
                abi.clone(),
                Contract::new(web3.eth(), governance_contract_eth_addr, abi.clone()),
            )
        };

        let franklin_contract = {
            let abi_string = serde_json::Value::from_str(models::abi::FRANKLIN_CONTRACT)
                .map_err(|e| format_err!("No franklin contract abi: {}", e.to_string()))?
                .get("abi")
                .ok_or_else(|| format_err!("No franklin contract abi"))?
                .to_string();
            let abi = ethabi::Contract::load(abi_string.as_bytes())
                .map_err(|e| format_err!("No franklin contract abi: {}", e.to_string()))?;
            (
                abi.clone(),
                Contract::new(web3.eth(), franklin_contract_eth_addr, abi.clone()),
            )
        };

        let mut events_state = EventsState::new();

        let genesis_franklin_transaction =
            get_ethereum_transaction(&web3, &franklin_contract_genesis_tx_hash)?;
        let genesis_governance_transaction =
            get_ethereum_transaction(&web3, &governance_contract_genesis_tx_hash)?;

        let genesis_eth_block_number =
            events_state.set_genesis_block_number(&genesis_governance_transaction)?;
        info!("genesis_eth_block_number: {:?}", &genesis_eth_block_number);

        storage_interactor::save_block_events_state(&connection_pool, &vec![])?;
        storage_interactor::save_last_wached_block_number(
            &connection_pool,
            genesis_eth_block_number,
        )?;

        let genesis_account = get_genesis_account(&genesis_franklin_transaction)?;

        let account_update = AccountUpdate::Create {
            address: genesis_account.address.clone(),
            nonce: genesis_account.nonce.clone(),
        };

        let mut account_map = AccountMap::default();
        account_map.insert(0, genesis_account.clone());

        let current_block = 0;
        let current_unprocessed_priority_op = 0;
        let fee_acc_num = 0;

        let tree_state = TreeState::load(
            current_block,
            account_map,
            current_unprocessed_priority_op,
            fee_acc_num,
        );

        info!("Genesis block number: {:?}", tree_state.state.block_number);
        info!("Genesis tree root hash: {:?}", tree_state.root_hash());
        debug!("Genesis accounts: {:?}", tree_state.get_accounts());

        storage_interactor::save_genesis_tree_state(&connection_pool, account_update)?;

        info!("Saved genesis tree state");

        Ok(Self {
            connection_pool,
            web3,
            governance_contract,
            franklin_contract,
            run_update: false,
            events_state,
            tree_state,
            eth_blocks_step,
            end_eth_blocks_offset,
        })
    }

    /// Stops states updates by setting run_update flag to false
    pub fn stop_state_update(&mut self) {
        self.run_update = false
    }

    /// Stops states from storage
    pub fn load_state_from_storage(&mut self) -> Result<(), failure::Error> {
        info!("Loading state from storage");
        let state = storage_interactor::get_storage_state(&self.connection_pool)?;
        self.events_state =
            storage_interactor::get_block_events_state_from_storage(&self.connection_pool)?;
        let tree_state = storage_interactor::get_tree_state(&self.connection_pool)?;
        self.tree_state = TreeState::load(
            tree_state.0, // current block
            tree_state.1, // account map
            tree_state.2, // unprocessed priority op
            tree_state.3, // fee account
        );
        match state {
            StorageUpdateState::Events => {
                // Update operations
                let new_ops_blocks = self.update_operations_state()?;
                // Update tree
                self.update_tree_state(new_ops_blocks)?;
            }
            StorageUpdateState::Operations => {
                // Update operations
                let new_ops_blocks =
                    storage_interactor::get_ops_blocks_from_storage(&self.connection_pool)?;
                // Update tree
                self.update_tree_state(new_ops_blocks)?;
            }
            StorageUpdateState::None => {}
        }
        info!(
            "State has been loaded, root hash: {:?}",
            self.tree_state.root_hash()
        );
        Ok(())
    }

    /// Activates states updates
    pub fn run_state_update(&mut self) -> Result<(), failure::Error> {
        self.run_update = true;
        let mut last_wached_block: u64 = self.events_state.last_watched_eth_block_number;
        while self.run_update {
            info!("Last watched ethereum block: {:?}", last_wached_block);

            // Update events
            let got_new_events = self.update_events_state()?;

            if got_new_events {
                // Update operations
                let new_ops_blocks = self.update_operations_state()?;

                // Update tree
                self.update_tree_state(new_ops_blocks)?;
            }

            if last_wached_block == self.events_state.last_watched_eth_block_number {
                std::thread::sleep(std::time::Duration::from_secs(5));
            } else {
                last_wached_block = self.events_state.last_watched_eth_block_number;
            }
        }
        info!("Stopped state updates");
        Ok(())
    }

    /// Updates events state, saves new blocks, tokens events and the last watched eth block number in storage
    /// Returns bool flag, true if there are new block events
    fn update_events_state(&mut self) -> Result<bool, failure::Error> {
        let (block_events, token_events, last_watched_eth_block_number) =
            self.events_state.update_events_state(
                &self.web3,
                &self.franklin_contract,
                &self.governance_contract,
                self.eth_blocks_step,
                self.end_eth_blocks_offset,
            )?;

        // Store block events
        storage_interactor::save_block_events_state(&self.connection_pool, &block_events)?;
        // Store block number
        storage_interactor::save_last_wached_block_number(
            &self.connection_pool,
            last_watched_eth_block_number,
        )?;
        // Store tokens
        storage_interactor::save_tokens(&self.connection_pool, token_events)?;

        storage_interactor::save_storage_state(&self.connection_pool, StorageUpdateState::Events)?;

        debug!("Updated events storage");

        Ok(!block_events.is_empty())
    }

    /// Updates tree state from the new Rollup operations blocks, saves it in storage
    ///
    /// # Arguments
    ///
    /// * `new_ops_blocks` - the new Rollup operations blocks
    ///
    fn update_tree_state(
        &mut self,
        new_ops_blocks: Vec<RollupOpsBlock>,
    ) -> Result<(), failure::Error> {
        let mut blocks = vec![];
        let mut updates = vec![];
        let mut count = 0;
        for op_block in new_ops_blocks {
            let (block, acc_updates) = self
                .tree_state
                .update_tree_states_from_ops_block(&op_block)?;
            blocks.push(block);
            updates.push(acc_updates);
            count += 1;
            info!(
                "New block number: {:?}",
                &self.tree_state.state.block_number
            );
            info!("Tree root hash: {:?}", self.tree_state.root_hash());
        }
        for i in 0..count {
            storage_interactor::update_tree_state(
                &self.connection_pool,
                blocks[i].clone(),
                updates[i].clone(),
            )?;
        }

        storage_interactor::save_storage_state(&self.connection_pool, StorageUpdateState::None)?;

        info!("Updated state\n");

        Ok(())
    }

    /// Gets new operations blocks from events, updates rollup operations stored state.
    /// Returns new rollup operations blocks
    fn update_operations_state(&mut self) -> Result<Vec<RollupOpsBlock>, failure::Error> {
        let new_blocks = self.get_new_operation_blocks_from_events()?;

        storage_interactor::save_rollup_ops(&self.connection_pool, &new_blocks)?;

        storage_interactor::save_storage_state(
            &self.connection_pool,
            StorageUpdateState::Operations,
        )?;

        debug!("Updated operations storage");

        Ok(new_blocks)
    }

    /// Returns verified comitted operations blocks from verified op blocks events
    pub fn get_new_operation_blocks_from_events(
        &mut self,
    ) -> Result<Vec<RollupOpsBlock>, failure::Error> {
        let committed_events = self.events_state.get_only_verified_committed_events();
        let mut blocks: Vec<RollupOpsBlock> = vec![];
        for event in committed_events {
            let mut _block = RollupOpsBlock::get_rollup_ops_block(&self.web3, &event)?;
            blocks.push(_block);
        }
        Ok(blocks)
    }
}
