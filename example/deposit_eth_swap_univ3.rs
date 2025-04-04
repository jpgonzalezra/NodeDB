use std::time::Instant;

use alloy::primitives::{address, Uint, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use eyre::{anyhow, Result};
use node_db::{InsertionType, NodeDB};
use revm::context::result::ExecutionResult;
use revm::primitives::{keccak256, TxKind};
use revm::{Context, ExecuteCommitEvm, ExecuteEvm, MainBuilder, MainContext};

sol! {
    #[sol(rpc)]
    contract WETH {
        function deposit() payable;
        function approve(address spender, uint256 amount) external returns (bool);
        function balanceOf(address account) external view returns (uint256);
    }

    #[sol(rpc)]
    contract UniswapV3Router {
        struct ExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint24 fee;
            address recipient;
            uint256 deadline;
            uint256 amountIn;
            uint256 amountOutMinimum;
            uint160 sqrtPriceLimitX96;
        }

        function exactInputSingle(ExactInputSingleParams calldata params) external returns (uint256 amountOut);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let account = address!("0000000000000000000000000000000000000001");
    let router = address!("E592427A0AEce92De3Edee1F18E0157C05861564"); // Uniswap V3 router
    let weth = address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let usdc = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

    let db_path = std::env::var("DB_PATH")?;
    let mut db = NodeDB::new(db_path)?;

    let start = Instant::now();

    let balance_slot = keccak256((account, U256::from(3)).abi_encode());
    db.insert_account_storage(
        weth,
        balance_slot.into(),
        U256::from(1e18),
        InsertionType::OnChain,
    )?;
    let deposit_calldata = WETH::depositCall {}.abi_encode();

    let mut evm = Context::mainnet()
        .with_db(&mut db)
        .modify_tx_chained(|tx| {
            tx.caller = account;
            tx.kind = TxKind::Call(weth);
            tx.data = deposit_calldata.into();
            tx.value = U256::from(1e18 as u128);
        })
        .build_mainnet();

    evm.replay_commit()?;

    let approve_calldata = WETH::approveCall {
        spender: router,
        amount: U256::from(10e18 as u128),
    }
    .abi_encode();

    let mut evm = Context::mainnet()
        .with_db(&mut db)
        .modify_tx_chained(|tx| {
            tx.caller = account;
            tx.kind = TxKind::Call(weth);
            tx.data = approve_calldata.into();
        })
        .build_mainnet();

    evm.replay_commit()?;

    let balanceof_calldata = WETH::balanceOfCall { account }.abi_encode();

    let mut evm = Context::mainnet()
        .with_db(&mut db)
        .modify_tx_chained(|tx| {
            tx.caller = address!("0000000000000000000000000000000000000000");
            tx.kind = TxKind::Call(weth);
            tx.data = balanceof_calldata.into();
        })
        .build_mainnet();

    let ref_tx = evm.replay()?;
    let balance = match ref_tx.result {
        ExecutionResult::Success { output, .. } => {
            let amount = U256::abi_decode(output.data(), false)?;
            println!("WETH balance: {}", amount);
            amount
        }
        result => return Err(anyhow!("balanceOf failed: {result:?}")),
    };

    let params = UniswapV3Router::ExactInputSingleParams {
        tokenIn: weth,
        tokenOut: usdc,
        fee: Uint::<24, 1>::from(3000),
        recipient: account,
        deadline: U256::MAX,
        amountIn: balance,
        amountOutMinimum: U256::ZERO,
        sqrtPriceLimitX96: Uint::<160, 3>::from(0),
    };

    let swap_calldata = UniswapV3Router::exactInputSingleCall { params }.abi_encode();

    let mut evm = Context::mainnet()
        .with_db(&mut db)
        .modify_tx_chained(|tx| {
            tx.caller = account;
            tx.kind = TxKind::Call(router);
            tx.data = swap_calldata.into();
        })
        .build_mainnet();

    let result = evm.replay_commit()?;
    match result {
        ExecutionResult::Success { output, .. } => {
            let out = U256::abi_decode(output.data(), false)?;
            println!(
                "Swapped {} WETH for {} USDC After Deposit and balanceOf in {:?}",
                balance,
                out,
                start.elapsed()
            );
        }
        result => return Err(anyhow!("swap failed: {result:?}")),
    };

    Ok(())
}
