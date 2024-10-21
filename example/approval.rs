use alloy::primitives::{address, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Result;
use node_db::{InsertionType, NodeDB};
use revm::primitives::keccak256;
use revm::wiring::default::TransactTo;
use revm::wiring::EthereumWiring;
use revm::Evm;

// Appoval and Allowance function signatures
sol!(
    #[sol(rpc)]
    contract ERC20Token {
        function approve(address spender, uint256 amount) external returns (bool success);
        function allowance(address owner, address spender) public view returns (uint256);
    }
);

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    // on chain addresses
    let account = address!("0000000000000000000000000000000000000001");
    let uniswap_router = address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

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
    evm.transact_commit().unwrap();

    // now confirm the allowance for the router
    let allowance_calldata = ERC20Token::allowanceCall {
        owner: account,
        spender: uniswap_router,
    }
    .abi_encode();
    evm.tx_mut().data = allowance_calldata.into();
    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;
    let output = result.output().unwrap();
    let allowance = <U256>::abi_decode(output, false).unwrap();
    println!("The router is allowed to spend {:?} weth", allowance);

    Ok(())
}
