# NodeDB
NodeDB is a revm DB implementation that hooks directly into the reth database and operates on the latest canonical state. The advantages of this are very obvious. Fetching from the db is **very** fast and should be used instead of RPC/IPC when its convenient. 

Current baked in revm DB implementations cache and use old state (CacheDB), fetch state from a static block number (AlloyDB), or cache and use old state while fetching from a static block number (CacheDB<AlloyDB>). 

# Usage
```rust
use node_db::NodeDB;

let nodedb = NodeDB::new("path/to/your/data")?;

//...use as you would any other db
```

