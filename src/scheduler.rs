use std::error::Error;
use crate::spider;
use crate::{extractor::Extractor, structer::Proxy};
use crate::storage::fetch_all_proxies;
use crate::validator::{process_proxies,store_checked_proxies};

async fn advanced_spider() -> Result<Vec<Proxy>, Box<dyn Error>> {

    let advanced_urls: Vec<String> = (1..=7).map(|page| {
        format!("https://advanced.name/freeproxy?page={page}")
    }).collect();
    
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
        },
        Err(e) => {
            eprintln!("{:#?}", e);
            Err(e.into())
        },
    }
}

async fn geonode_spider() -> Result<Vec<Proxy>, Box<dyn Error>> {
    let proxylist_urls: Vec<String> = (0..=15).map(|page| {
        format!("https://proxylist.geonode.com/api/proxy-list?limit=500&page={page}&sort_by=lastChecked&sort_type=desc")
    }).collect();
    let mut proxies: Vec<Proxy> = Vec::new();
    let spider = spider::Spider::new(proxylist_urls).await;
    match spider.fetch().await {
        Ok(bodies) => {
            for body in bodies {
                let extractor = Extractor::new();
                let mut p = extractor.extract_proxies_geonode(&body)?;
                proxies.append(&mut p);
            }
            return Ok(proxies);
        },
        Err(e) => {
            eprintln!("{:#?}", e);
            return Err(e.into());
        },
    }
}

pub async fn all_spider_running() -> Result<Vec<Proxy>, Box<dyn Error>> {
    let mut proxies = advanced_spider().await?;
    let mut geo = geonode_spider().await?;
    proxies.append(&mut geo);
    Ok(proxies)
}

pub async fn all_enginer_running() -> Result<(), Box<dyn Error>> {
    let mut new_proxies = all_spider_running().await?;
    println!("load new spider data => {:#?}", new_proxies.len());
    new_proxies.sort_by(|a, b| a.ip.cmp(&b.ip).then_with(|| a.port.cmp(&b.port)));
    new_proxies.sort_by(|a, b| b.last_checked.cmp(&a.last_checked));
    new_proxies.dedup_by(|a, b| a.ip == b.ip && a.port == b.port);
    println!("after dedup spider data => {:#?}", new_proxies.len());
    let mut old_proxies = fetch_all_proxies()?;
    let mut proxy_map: hashbrown::HashMap<(String, String), Proxy> = old_proxies
    .drain(..) // 移动 old_proxies 中的所有元素
    .map(|p| ((p.ip.clone(), p.port.clone()), p))
    .collect();

    for proxy in new_proxies {
        match proxy_map.get_mut(&(proxy.ip.clone(), proxy.port.clone())) {
            Some(existing_proxy) => {
                existing_proxy.last_checked = proxy.last_checked;
            },
            None => {
                proxy_map.insert((proxy.ip.clone(), proxy.port.clone()), proxy);
            },
        }
    }
    let merged_proxies: Vec<Proxy> = proxy_map.into_values().collect();
    println!("merge spider data with db data => {:#?}", merged_proxies.len());
    let checked_proxies = process_proxies(merged_proxies).await;
    store_checked_proxies(checked_proxies).await;
    Ok(())
}

