use crate::config::auto_detect;
use std::time::Instant;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;

pub async fn run() {
    let config = auto_detect();
    println!("{:<15} | {:<10} | {:<10} | {:<8} | {:<8}", "Server", "Start", "Ping", "Tools", "RAM");
    println!("{:-<15}-|-{:-<10}-|-{:-<10}-|-{:-<8}-|-{:-<8}", "", "", "", "", "");

    let manager = std::sync::Arc::new(crate::child::ChildManager::new(
        config.servers.clone(),
        300_000,
    ));

    let mut names: Vec<_> = config.servers.keys().cloned().collect();
    names.sort();

    for name in names {
        let start_time = Instant::now();
        let tools_res = manager.start_server(&name).await;
        let start_duration = start_time.elapsed().as_millis();

        if let Ok(tools) = tools_res {
            let ping_start = Instant::now();
            let _ = manager.call_method(&name, "ping", serde_json::json!({})).await;
            let ping_duration = ping_start.elapsed().as_millis();

            // Placeholder for RAM since accurate process tree measuring is complex in Rust without sysinfo crate
            let ram = "N/A"; 

            println!("{:<15} | {:<8}ms | {:<8}ms | {:<8} | {:<8}", 
                name, start_duration, ping_duration, tools.len(), ram);
        } else {
            println!("{:<15} | {:<10} | {:<10} | {:<8} | {:<8}", name, "FAILED", "-", "-", "-");
        }
    }
    
    manager.stop_all().await;
}