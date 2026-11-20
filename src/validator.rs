use crate::storage::{live_proxies_db_update, store_proxies};
use crate::structer::Proxy;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

pub fn proxy_scheme(proxy_types: &str, force_https: bool) -> String {
    let proxy_types = proxy_types.to_lowercase();
    let mut proxy_scheme = proxy_types.split(", ").collect::<Vec<_>>()[0];
    if proxy_scheme == "socks4" {
        proxy_scheme = "socks5"
    };
    if force_https && proxy_types.contains("https") {
        proxy_scheme = "https"
    };
    proxy_scheme.to_string()
}

use tokio::net::TcpStream;
use tokio::time::timeout;

pub async fn has_internet_connection_async() -> bool {
    let timeout_duration = Duration::from_secs(2);

    match timeout(timeout_duration, TcpStream::connect("1.1.1.1:53")).await {
        Ok(Ok(_stream)) => true, // Соединение установлено успешно
        _ => false,              // Таймаут или ошибка сети
    }
}

pub static TEST_URL: OnceLock<String> = OnceLock::new();

pub async fn check_proxy_alive(proxy: &Proxy) -> bool {
    let proxy_type = proxy_scheme(&proxy.proxy_type, false);
    let proxy_url = format!("{}://{}:{}", proxy_type, proxy.ip, proxy.port);
    let client = Client::builder()
        .proxy(
            reqwest::Proxy::all(&proxy_url).expect(&format!("Client builder, proxy: {proxy_url}")),
        )
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    while !has_internet_connection_async().await {
        //println!("No internet connection, retrying in 5 seconds...");
        //print!(".");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let test_url = TEST_URL.get_or_init(|| "https://httpbin.org/ip".into());
    let is_test_url = test_url == "https://httpbin.org/ip";

    match client
        .get(test_url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => {
            let mut ret = response.status().is_success();
            //let _status = response.status();
            let mut body = match response.text().await {
                Ok(b) => b,
                Err(_e) => return false,
            };
            if ret && is_test_url {
                body = body.replace(" ", "").replace("\n", "");
                let proxy_ip = format!(r#""origin":"{}""#, proxy.ip);
                //ret = body.contains(&proxy_ip);
                ret = body.contains(r#""origin": ""#);
                dbg!(proxy_ip);
            } else {
                let body2 = body.trim_start();
                body = body2[..std::cmp::min(12, body2.len())].to_owned();
                //dbg!(ret, body);
            }
            //ret = body.contains(r#""origin": ""#);
            if ret {
                print!("{proxy_url} {:?} {body}", proxy);
                //print!(" body: {}", response.text().await.unwrap_or_default());
                println!();
            }
            ret
        }
        Err(_e) => {
            //println!("{:?} {:?}", proxy, _e);
            false
        }
    }
}

// Стало (принимает Vec, HashSet, итераторы и т.д.):
pub async fn process_proxies<I>(proxies: I) -> Vec<Proxy>
where
    I: IntoIterator<Item = Proxy>,
{
    let check_futures = stream::iter(proxies.into_iter().map(|proxy| async move {
        let alive = check_proxy_alive(&proxy).await;
        let last_checked = if alive {
            Utc::now().format("%Y.%m.%d %H:%M:%S").to_string()
        } else {
            proxy.last_checked
        };
        Proxy {
            ip: proxy.ip,
            port: proxy.port,
            proxy_type: proxy.proxy_type.to_lowercase(),
            country: proxy.country.to_uppercase(),
            last_checked,
            check_number: proxy.check_number + 1,
            live_number: proxy.live_number + alive as u64,
            live: alive,
        }
    }));

    check_futures.buffer_unordered(100).collect().await
}

pub async fn store_checked_proxies(check_proxies: Vec<Proxy>) {
    println!("inserting all discovery proxy to db...");

    tokio::task::spawn_blocking(|| {
        store_proxies(check_proxies).unwrap();
    })
    .await
    .expect("Failed to execute store_proxies");

    println!("inserting all live proxy to db...");

    tokio::task::spawn_blocking(|| {
        let cnt = live_proxies_db_update().unwrap();
        println!("Alive proxies - {cnt}");
    })
    .await
    .expect("Failed to execute live_proxies_db_update");

    println!("All db stored, bye !!!")
}
