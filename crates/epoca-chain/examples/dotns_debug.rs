fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    let name = std::env::args().nth(1).unwrap_or_else(|| "hackme3".to_string());
    println!("=== Resolving: {name} ===\n");

    match epoca_chain::dotns::resolve_dotns(&name) {
        Ok(cid) => {
            println!("CID: {cid}\n");

            println!("=== Fetching IPFS content ===\n");
            match epoca_chain::dotns::fetch_ipfs(&cid) {
                Ok(assets) => {
                    println!("Total assets: {}", assets.len());
                    for (k, v) in &assets {
                        println!("  {k} ({} bytes)", v.len());
                    }
                    if let Some(html) = assets.get("index.html") {
                        let s = std::str::from_utf8(html).unwrap_or("<binary>");
                        println!("\n--- index.html preview (first 1000 chars) ---");
                        println!("{}", &s[..s.len().min(1000)]);
                    }
                }
                Err(e) => println!("Fetch error: {e}"),
            }
        }
        Err(e) => println!("Resolve error: {e}"),
    }
}
