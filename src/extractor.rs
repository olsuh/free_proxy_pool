use crate::structer::{GeonodeProxy, GeonodeProxyData, Proxy};
use base64::{engine::general_purpose, Engine as _};
use scraper::{Html, Selector};
use serde_json;
use std::{error::Error, str};

pub struct Extractor;

impl Extractor {
    pub fn new() -> Self {
        Extractor
    }

    pub fn extract_proxies_advanced(&self, html: &str) -> Result<Vec<Proxy>, Box<dyn Error>> {
        let mut proxies: Vec<Proxy> = Vec::with_capacity(100);
        let mut debug: Option<&str> = None; // Для отладки можно установить Some("DEBUG") или None для отключения
        let document = Html::parse_document(html);

        // Выносим все селекторы за пределы циклов для оптимизации
        let tbody_selector = Selector::parse("tbody").unwrap();
        let tr_selector = Selector::parse("tr").unwrap();
        let td_selector = Selector::parse("td").unwrap();

        // Селекторы для старого варианта (Base64 в атрибутах)
        let old_ip_sel = Selector::parse("td[data-ip]").unwrap();
        let old_port_sel = Selector::parse("td[data-port]").unwrap();
        let old_type_sel = Selector::parse("td a.label").unwrap();

        // Селекторы для нового варианта (открытый текст)
        let new_ip_sel = Selector::parse("td.ip").unwrap();
        let new_type_sel = Selector::parse("td.types a.tag").unwrap();

        // Общий селектор страны
        let country_sel = Selector::parse("td a[href*='country=']").unwrap();

        for tbody in document.select(&tbody_selector) {
            for tr in tbody.select(&tr_selector) {
                // 1. Определяем ветку и извлекаем IP и Port
                let (ip, port, is_new_format) = if let (Some(ip_el), Some(port_el)) = (
                    tr.select(&old_ip_sel).next(),
                    tr.select(&old_port_sel).next(),
                ) {
                    if debug.is_none() {
                        debug = Some(
                            "[DEBUG] Ветка 1 (Старый формат): найдены атрибуты data-ip и data-port",
                        );
                    }

                    let ip_b64 = ip_el.value().attr("data-ip").unwrap_or_default();
                    let port_b64 = port_el.value().attr("data-port").unwrap_or_default();

                    let ip_bytes = general_purpose::STANDARD.decode(ip_b64)?;
                    let port_bytes = general_purpose::STANDARD.decode(port_b64)?;

                    let ip = String::from_utf8(ip_bytes)?;
                    let port = String::from_utf8(port_bytes)?;

                    (ip, port, false)
                } else if let Some(ip_el) = tr.select(&new_ip_sel).next() {
                    if debug.is_none() {
                        debug = Some("[DEBUG] Ветка 2 (Новый формат): найдена ячейка td.ip");
                    }

                    let ip = ip_el.text().collect::<String>().trim().to_string();

                    // В новом формате порт идёт в третьем <td> (индекс 2: №, IP, Port)
                    let port = tr
                        .select(&td_selector)
                        .nth(2)
                        .map(|td| td.text().collect::<String>().trim().to_string())
                        .unwrap_or_default();

                    (ip, port, true)
                } else {
                    if debug.is_none() {
                        debug =
                            Some("[DEBUG] Ветка 3 (Пропуск): строка не подошла ни под один формат");
                    }
                    continue;
                };

                // 2. Извлекаем типы прокси в зависимости от ветки
                let type_selector = if is_new_format {
                    &new_type_sel
                } else {
                    &old_type_sel
                };
                let proxy_types: Vec<String> = tr
                    .select(type_selector)
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .collect();
                let proxy_type = proxy_types.join(", ");

                // 3. Извлекаем код страны (работает одинаково для обеих веток)
                let country = tr
                    .select(&country_sel)
                    .next()
                    .map(|n| n.text().collect::<String>().trim().to_string())
                    .unwrap_or_default();

                // 4. Извлекаем время последней проверки из последней ячейки <td>
                let last_checked_raw = tr
                    .select(&td_selector)
                    .last()
                    .map(|e| e.text().collect::<String>().trim().to_string())
                    .unwrap_or_default();
                let last_checked = format_time_advanced(&last_checked_raw);

                /*println!(
                    "[DEBUG] Итог -> IP: {}, Port: {}, Type: [{}], Country: {}, Checked: {}",
                    ip, port, proxy_type, country, last_checked
                );*/

                proxies.push(Proxy {
                    ip,
                    port,
                    proxy_type,
                    country,
                    last_checked,
                    check_number: 0,
                    live_number: 0,
                    live: false,
                });
            }
        }
        if let Some(debug_msg) = debug {
            println!("{}", debug_msg);
        }
        Ok(proxies)
    }

    pub fn extract_proxies_geonode(&self, html: &str) -> Result<Vec<Proxy>, Box<dyn Error>> {
        let proxies_geonode_data: GeonodeProxyData =
            serde_json::from_str::<GeonodeProxyData>(html)?;
        let proxies_geonode: Vec<GeonodeProxy> = proxies_geonode_data.data;
        let mut proxies: Vec<Proxy> = Vec::with_capacity(proxies_geonode.len());
        for proxy in proxies_geonode {
            let ip = proxy.ip;
            let port = proxy.port;
            let port = port.unwrap_or_else(|| "-".to_string());
            let proxy_type = proxy.protocols.join(",");
            let country = proxy.country;
            let country = country.unwrap_or_else(|| "-".to_string());
            let last_checked = proxy.updated_at;
            let last_checked = last_checked.unwrap_or_else(|| "-".to_string());
            let proxy = Proxy {
                ip,
                port,
                proxy_type,
                country,
                last_checked,
                check_number: 0,
                live_number: 0,
                live: false,
            };
            proxies.push(proxy);
        }

        Ok(proxies)
    }
}

fn format_time_advanced(input: &str) -> String {
    // 首先，移除"<small>(...)</small>"部分
    let clean_input = input.split("<small>").next().unwrap_or(input).trim();
    // 然后，调整时间格式
    if let Some((time, date)) = clean_input.split_once(' ') {
        // 假设日期格式是 dd.mm.yyyy，我们需要将其转换为 yyyy.mm.dd
        let date_parts: Vec<&str> = date.split('.').collect();
        if date_parts.len() == 3 {
            return format!(
                "{}.{}.{} {}",
                date_parts[2], date_parts[1], date_parts[0], time
            );
        }
    }
    input.to_string()
}
