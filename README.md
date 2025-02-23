# NodeDB
NodeDB is a revm DB implementation that hooks directly into the reth database and operates on the latest canonical state. The advantages of this are very obvious. Fetching from the db is **very** fast and should be used instead of RPC/IPC when possible. 

Current baked in revm DB implementations cache state without fetching from provider (CacheDB), fetch state from provider but dont dont allow you to insert accounts/storage (AlloyDB), or fetch state and cache it which results in old state (CacheDB(AlloyDB)). 

# Note
If you are running reth 1.2, you must enable immediate block flushing via `--engine.persistence-threshold 0` and `--engine.memory-block-buffer-target 0`

# Usage
```rust
use node_db::NodeDB;

let nodedb = NodeDB::new("path/to/your/data")?;

//...use as you would any other db
```

# Use Case/Inspiration
I have a custom quoter contract for my arbitrage bot which I use for simulations. I needed a DB that would allow me to insert the contract code and other state information while also being able to operate on the most up to date chain state for proper quotes. NodeDB allows me to insert and execute against arbitraty contract code while also being able to insert storage data for things such as token balances, approvals, etc. 

# Problems

We have no way to know before hand if an account/storage has been updated. The only way is to fetch the account when it is needed and then compare it to the cached value. If we are fetching, we might as well just insert it anyways. On the other hand, we know that custom contracts/accounts that we have inserted will only be modifed within the evm instance as it has no chain state. 

You could implement functionality to pass in list of updated accounts that can be marked as out of date, or you could modify the `update_provider` function to process blocks and determine touched state.

This is not the prettiest implementation and im sure there are uncaught edge cases, but it works for what I need to accomplish. Please fork/submit pull req if you you make any positive upgrades! 
