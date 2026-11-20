use crate::storage::initialize_db;
use async_recursion::async_recursion;
use scheduler::all_engine_running;
use std::error::Error;
use tokio::time::{sleep, Duration};

mod extractor;
mod scheduler;
/**
page =>
   https://advanced.name/freeproxy?page=1
   https://github.com/TheSpeedX/PROXY-List
*/
mod spider;
mod storage;
pub mod structer;
mod validator;

#[async_recursion]
async fn run_periodically() -> Result<(), Box<dyn Error>> {
    // Запуск задач
    match all_engine_running().await {
        Ok(_) => println!("Task completed successfully. wait 10 minutes"),
        Err(e) => eprintln!("Task failed: {}", e),
    }
    sleep(Duration::from_secs(60 * 10)).await;
    run_periodically().await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args();
    args.skip(1).next().and_then(|arg| {
        validator::TEST_URL.set(arg.clone()).ok();
        println!("Set TEST_URL to: {}", arg);
        Some(())
    });
    initialize_db()?;
    run_periodically().await
}
