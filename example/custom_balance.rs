use alloy::primitives::{address, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use eyre::Result;
use node_db::{InsertionType, NodeDB};
use revm::primitives::keccak256;
use revm::wiring::default::TransactTo;
use revm::wiring::EthereumWiring;
use revm::Evm;

// function signature
sol!(
    #[sol(rpc)]
    contract ERC20Token {
        function balanceOf(address account) public view returns (uint256);
    }
);

#[tokio::main]
async fn main() -> Result<()> {
    // on chain addresses
    let account = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");

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

    // setup our balance_of calldata
    let balance_calldata = ERC20Token::balanceOfCall { account }.abi_encode();

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
            tx.data = balance_calldata.into();
            tx.transact_to = TransactTo::Call(weth);
        })
        .build();
    let ref_tx = evm.transact().unwrap();
    let result = ref_tx.result;
    let output = result.output().unwrap();
    let balance = <U256>::abi_decode(output, false).unwrap();
    println!("Account has custom balance {:?}", balance);
    Ok(())
}

