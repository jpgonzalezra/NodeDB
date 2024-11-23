use alloy::primitives::{address, U256};
use alloy::sol_types::{SolCall, SolValue};
use eyre::Result;
use alloy::sol;
use node_db::{InsertionType, NodeDB};
use revm::primitives::{keccak256, TransactTo};
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
    dotenv::dotenv().ok();

    // on chain addresses
    let account = address!("0000000000000000000000000000000000000001");
    let uniswap_router = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

    // construct the database
    let database_path = std::env::var("DB_PATH").unwrap().parse().unwrap();
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
    let mut evm = Evm::builder()
        .with_db(&mut nodedb)
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
    evm.transact_commit().unwrap();

    // we now have some of the input token and we have approved the router to spend it

    // setup calldata based on the swap type
    let calldata = V2Swap::swapExactTokensForTokensCall {
        amountIn: U256::from(1e18),
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
    let output = result.output().unwrap();
    let decoded_outputs = <Vec<U256>>::abi_decode(output, false).unwrap();
    println!(
        "Swapped 1 WETH for {} USDC",
        decoded_outputs.get(1).unwrap()
    );

    Ok(())
}
