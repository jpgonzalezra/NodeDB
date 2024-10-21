use alloy::primitives::{address, Address, U160, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use alloy::transports::http::Client as AlloyClient;
use alloy::transports::http::Http;
use revm::database_interface::WrapDatabaseAsync;
use revm::primitives::keccak256;
use revm::wiring::default::TransactTo;
use revm::wiring::result::ExecutionResult;
use revm::wiring::EthereumWiring;
use revm::Evm;
use node_db::{NodeDB, InsertionType};
use eyre::Result;

sol!(
    #[sol(rpc)]
    contract V2Swap {
        function swapExactTokensForTokens(
            uint256 amountIn,
            uint256 amountOutMin,
            address[] calldata path,
            address to,
            uint256 deadline
        ) external returns (uint256[] memory amounts);
    }
);

sol!(
    #[sol(rpc)]
    contract ERC20Token {
        function approve(address spender, uint256 amount) external returns (bool success);
    }
);

fn main() -> Result<()> {

    // state
    let account = address!("0000000000000000000000000000000000000001");
    let balance_slot = keccak256((account, U256::from(3)).abi_encode());
    let router_address = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let weth_usdc_pool = address!("88A43bbDF9D098eEC7bCEda4e2494615dfD9bB9C");

    let database_path = String::from("/mnt/eth-docker-data");
    let mut nodedb = NodeDB::new(database_path).unwrap();
    // construct the db

    nodedb.insert_account_storage(weth, balance_slot.into(), U256::from(1e18), InsertionType::Fetched)?;

        // construct a new evm instance
    let mut evm = Evm::<EthereumWiring<&mut NodeDB, ()>>::builder()
        .with_db(&mut nodedb)
        .with_default_ext_ctx()
        .modify_cfg_env(|env| {
            env.disable_nonce_check = true;
        })
        .modify_tx_env(|tx| {
            tx.caller = account;
            tx.value = U256::ZERO;
        })
        .build();

    // setup approval call and transact
    let approve_calldata = ERC20Token::approveCall {
        spender: router_address,
        amount: U256::from(10e18),
    }
    .abi_encode();
    evm.tx_mut().transact_to = TransactTo::Call(weth);
    evm.tx_mut().data = approve_calldata.into();
    let res = evm.transact_commit().unwrap();
    println!("{:?}", res);

    /* 
        // we now have some of the input token and we have approved the router to spend it
        // try a swap to see if if it is valid

        // setup calldata based on the swap type
        let calldata =  V2Swap::swapExactTokensForTokensCall {
            amountIn: U256::from(1e16),
            amountOutMin: U256::ZERO,
            path: vec![pool.token0_address(), pool.token1_address()],
            to: account,
            deadline: U256::MAX,
        }
        .abi_encode(),

        // set call to the router
        evm.tx_mut().transact_to = TransactTo::Call(address);
        evm.tx_mut().data = calldata.into();

        // if we can transact, add it as it is a valid pool. Else ignore it
        let ref_tx = evm.transact().unwrap();
        let result = ref_tx.result;
        //println!("{:#?}", ref_tx);
        if let ExecutionResult::Success { .. } = result {
            debug!("Successful swap for pool {}", pool.address());
            filtered_pools.push(pool.clone());
        } else {
            debug!("Unsuccessful swap for pool {}", pool.address());
        }
    }
    */
    Ok(())
}
