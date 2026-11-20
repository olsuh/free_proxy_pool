use crate::spider;
use crate::storage::fetch_all_proxies;
use crate::validator::{process_proxies, store_checked_proxies};
use crate::{extractor::Extractor, structer::Proxy};
use std::error::Error;

async fn advanced_spider() -> Result<Vec<Proxy>, Box<dyn Error>> {
    let advanced_urls: Vec<String> = (1..=7)
        .map(|page| format!("https://advanced.name/freeproxy?page={page}"))
        .collect();

    let mut proxies: Vec<Proxy> = Vec::new();
    let spider = spider::Spider::new(advanced_urls).await;
    match spider.fetch().await {
        Ok(bodies) => {
            for body in bodies {
                let extractor = Extractor::new();
                let mut p = extractor.extract_proxies_advanced(&body)?;
                proxies.append(&mut p);
            }
            Ok(proxies)
        }
        Err(e) => {
            eprintln!("{:#?}", e);
            Err(e.into())
        }
    }
}

async fn geonode_spider() -> Result<Vec<Proxy>, Box<dyn Error>> {
    let proxylist_urls: Vec<String> = (0..=15).map(|page| {
        format!("https://proxylist.geonode.com/api/proxy-list?limit=500&page={page}&sort_by=lastChecked&sort_type=desc")
    }).collect();

    let spider = spider::Spider::new(proxylist_urls).await;
    match spider.fetch().await {
        Ok(bodies) => {
            let mut proxies: Vec<Proxy> = Vec::with_capacity(16 * 500);
            for body in bodies {
                let extractor = Extractor::new();
                let mut p = extractor.extract_proxies_geonode(&body)?;
                proxies.append(&mut p);
            }
            return Ok(proxies);
        }
        Err(e) => {
            eprintln!("{:#?}", e);
            return Err(e.into());
        }
    }
}

pub async fn all_spider_running() -> Result<Vec<Proxy>, Box<dyn Error>> {
    let mut proxies = advanced_spider().await?;
    let mut geo = geonode_spider().await?;
    proxies.append(&mut geo);
    Ok(proxies)
}

use hashbrown::HashSet;

pub async fn all_engine_running() -> Result<(), Box<dyn Error>> {
    let new_proxies = all_spider_running().await?;
    println!("load new spider data => {}", new_proxies.len());

    // 2. Загружаем прокси из БД сразу в HashSet без клонирования строк
    let mut proxy_set: HashSet<Proxy> = fetch_all_proxies()?.into_iter().collect();
    println!("db proxies => {}", proxy_set.len());

    // 3. Сливаем данные: обновляем last_checked у существующих, добавляем новые
    for new_proxy in new_proxies {
        if let Some(mut existing_proxy) = proxy_set.take(&new_proxy) {
            if new_proxy.last_checked > existing_proxy.last_checked {
                existing_proxy.last_checked = new_proxy.last_checked;
            }
            proxy_set.insert(existing_proxy);
        } else {
            proxy_set.insert(new_proxy);
        }
    }

    println!("merge spider data with db data => {}", proxy_set.len());

    let checked_proxies = process_proxies(proxy_set).await;
    store_checked_proxies(checked_proxies).await;
    Ok(())
}
