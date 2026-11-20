#[path = "../src/structer.rs"]
mod structer;

use structer::Proxy;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use hashbrown::{hash_map::RawEntryMut, HashMap, HashSet};
use std::hash::Hasher;

// Замените на импорт из вашего крейта, если Proxy уже реализует Hash и Eq по (ip, port)

// Вариант 1: hashbrown::HashSet через get_mut + insert
/*fn merge_hashset_get_mut(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_set: HashSet<Proxy> = old_proxies.into_iter().collect();

    for new_proxy in new_proxies {
        if let Some(existing_proxy) = proxy_set.get_mut(&new_proxy) {
            if new_proxy.last_checked > existing_proxy.last_checked {
                existing_proxy.last_checked = new_proxy.last_checked;
            }
        } else {
            proxy_set.insert(new_proxy);
        }
    }

    proxy_set.into_iter().collect()
}
*/
fn merge_hashset_take_insert(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_set: HashSet<Proxy> = old_proxies.into_iter().collect();

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

    proxy_set.into_iter().collect()
}

// Вариант 2: hashbrown::HashMap<Proxy, ()> через raw_entry_mut (строго 1 поиск по хэшу)
fn merge_hashmap_raw_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_map: HashMap<Proxy, ()> = old_proxies.into_iter().map(|p| (p, ())).collect();

    for new_proxy in new_proxies {
        match proxy_map.raw_entry_mut().from_key(&new_proxy) {
            RawEntryMut::Occupied(mut entry) => {
                let existing_proxy = entry.key_mut();
                if new_proxy.last_checked > existing_proxy.last_checked {
                    existing_proxy.last_checked = new_proxy.last_checked;
                }
            }
            RawEntryMut::Vacant(entry) => {
                entry.insert(new_proxy, ());
            }
        }
    }

    proxy_map.into_keys().collect()
}

/// Генерирует тестовые данные с заданным процентом пересечения (overlap_ratio: 0.0 ..= 1.0)
fn generate_data(count: usize, overlap_ratio: f64) -> (Vec<Proxy>, Vec<Proxy>) {
    let make_proxy = |i: usize, date: &str| Proxy {
        ip: format!("192.168.{}.{}", (i / 255) % 255, i % 255),
        port: (8000 + (i % 1000)).to_string(),
        proxy_type: "http".to_string(),
        country: "UA".to_string(),
        last_checked: date.to_string(),
        check_number: 1,
        live_number: 1,
        live: true,
    };

    let old: Vec<Proxy> = (0..count)
        .map(|i| make_proxy(i, "2026.11.19 10:00:00"))
        .collect();

    let offset = ((1.0 - overlap_ratio) * count as f64) as usize;
    let new: Vec<Proxy> = (offset..offset + count)
        .map(|i| make_proxy(i, "2026.11.20 01:00:00"))
        .collect();

    (old, new)
}

fn bench_proxy_merge(c: &mut Criterion) {
    let count = 50_000;
    let scenarios = [
        ("90%_duplicates", 0.9),
        ("50%_duplicates", 0.5),
        ("10%_duplicates_mostly_new", 0.1),
    ];

    let mut group = c.benchmark_group("Proxy_Merge_50k");

    for (name, overlap) in scenarios {
        let (old, new) = generate_data(count, overlap);

        group.bench_with_input(
            BenchmarkId::new("hashset_take_insert", name),
            &(),
            |b, _| {
                b.iter_batched(
                    || (old.clone(), new.clone()),
                    |(o, n)| black_box(merge_hashset_take_insert(o, n)),
                    BatchSize::LargeInput,
                );
            },
        );

        group.bench_with_input(BenchmarkId::new("HashMap_raw_entry", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_hashmap_raw_entry(o, n)),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_proxy_merge);
criterion_main!(benches);
