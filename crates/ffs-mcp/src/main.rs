//! `ffs-mcp`: MCP server exposing the six MVP tools as a thin wrapper over
//! the daemon's JSON-RPC. Tool implementations are introduced by task 16.

fn main() {
    println!("ffs-mcp scaffold");
}

#[cfg(test)]
mod tests {
    #[test]
    fn smoke() {
        assert_eq!(ffs_core::CRATE_NAME, "ffs-core");
    }
}
