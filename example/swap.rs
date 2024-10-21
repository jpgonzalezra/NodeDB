use alloy::primitives::{address, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Result;
use node_db::{InsertionType, NodeDB};
use revm::primitives::keccak256;
use revm::wiring::default::TransactTo;
use revm::wiring::result::ExecutionResult;
use revm::wiring::EthereumWiring;
use revm::Evm;

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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    // on chain addresses
    let account = address!("0000000000000000000000000000000000000001");
    let uniswap_router = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

    // construct the database
    let database_path = String::from("/mnt/eth-docker-data");
    let mut nodedb = NodeDB::new(database_path).unwrap();

    // give our account some weth
    let balance_slot = keccak256((account, U256::from(3)).abi_encode());
    nodedb.insert_account_storage(
        weth,
        balance_slot.into(),
        U256::from(1e18),
        InsertionType::OnChain, // weth has a corresponding onchain contract
    )?;

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

    // setup approval call and commit the transaction
    let approve_calldata = ERC20Token::approveCall {
        spender: uniswap_router,
        amount: U256::from(10e18),
    }
    .abi_encode();
    evm.tx_mut().transact_to = TransactTo::Call(weth);
    evm.tx_mut().data = approve_calldata.into();
    let res = evm.transact_commit().unwrap();
    println!("{:?}", res);

    // we now have some of the input token and we have approved the router to spend it
    // try a swap to see if if it is valid

    // setup calldata based on the swap type
    let calldata = V2Swap::swapExactTokensForTokensCall {
        amountIn: U256::from(1e16),
        amountOutMin: U256::ZERO,
        path: vec![weth, usdc],
        to: account,
        deadline: U256::MAX,
    }
    .abi_encode();

    // set call to the router
    evm.tx_mut().transact_to = TransactTo::Call(uniswap_router);
    evm.tx_mut().data = calldata.into();

    // if we can transact, add it as it is a valid pool. Else ignore it
    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;
    println!("{:#?}", result);
    if let ExecutionResult::Success { .. } = result {
        println!("success");
    } else {
        println!("not success");
    }
    Ok(())
}
