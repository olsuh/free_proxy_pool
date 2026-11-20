#[path = "../src/structer.rs"]
mod structer;

use structer::Proxy;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use hashbrown::{hash_map::RawEntryMut, HashMap as HbHashMap, HashSet as HbHashSet};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

// ВАЖНО: для RawEntryMut нужен feature в Cargo.toml:
//   hashbrown = { version = "0.15", features = ["raw"] }
// И ВАЖНО: Hash/PartialEq у Proxy должны учитывать ТОЛЬКО (ip, port).
// Если они выведены derive и включают last_checked — слияние молча не сработает.
//
// ВАЖНО: эта версия требует, чтобы Proxy: Clone + Eq + Hash (по ip, port).
// Реализация в structer.rs должна быть такой:
//
// impl Hash for Proxy {
//     fn hash<H: Hasher>(&self, state: &mut H) {
//         self.ip.hash(state);
//         self.port.hash(state);
//     }
// }
//
// impl PartialEq for Proxy {
//     fn eq(&self, other: &Self) -> bool {
//         self.ip == other.ip && self.port == other.port
//     }
// }
//
// impl Eq for Proxy {}

// Вариант 1: hashbrown::HashSet через take + insert (2 поиска по хешу)
fn merge_hashset_take_insert(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_set: HbHashSet<Proxy> = old_proxies.into_iter().collect();

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

// Вариант 2: hashbrown::HashMap<Proxy, ()> через raw_entry_mut (строго 1 поиск по хешу)
fn merge_hashmap_raw_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_map: HbHashMap<Proxy, ()> = old_proxies.into_iter().map(|p| (p, ())).collect();

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

// Базлайн: std HashMap<(String, String), Proxy> через entry
fn merge_std_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HashMap<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();

    for new_proxy in new_proxies {
        match map.entry((new_proxy.ip.clone(), new_proxy.port.clone())) {
            Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if new_proxy.last_checked > existing.last_checked {
                    existing.last_checked = new_proxy.last_checked;
                }
            }
            Entry::Vacant(e) => {
                e.insert(new_proxy);
            }
        }
    }

    map.into_values().collect()
}

/// Ожидаемая длина результата слияния при заданном проценте пересечения.
/// При overlap = 1.0: |merged| = count. При overlap = 0.0: |merged| = 2 * count.
fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    let old_unique = count;
    let new_unique = ((1.0 - overlap_ratio) * count as f64) as usize;
    old_unique + new_unique
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

/// Разовая проверка корректности: без неё бенчмарк может мерить
/// скорость неправильно работающего кода (например, при derive(Hash)
/// по всем полям Proxy слияние превратится в «вставить всё как новое»).
fn sanity_check(count: usize, overlap: f64) {
    let (old, new) = generate_data(count, overlap);
    let expected = expected_merged_len(count, overlap);

    let r1 = merge_hashset_take_insert(old.clone(), new.clone());
    assert_eq!(r1.len(), expected, "hashset_take_insert: wrong merged len");

    let r2 = merge_hashmap_raw_entry(old.clone(), new.clone());
    assert_eq!(r2.len(), expected, "hashmap_raw_entry: wrong merged len");

    let r3 = merge_std_entry(old.clone(), new.clone());
    assert_eq!(r3.len(), expected, "std_entry: wrong merged len");

    // Согласованность вариантов: последний last_checked у одного и того же
    // (ip, port) должен быть одинаковым во всех реализациях.
    for (a, b) in r1.iter().zip(r2.iter()) {
        assert_eq!(a.last_checked, b.last_checked);
    }
}

fn bench_proxy_merge(c: &mut Criterion) {
    const COUNT: usize = 50_000;
    const SCENARIOS: [(&str, f64); 4] = [
        ("90%_duplicates", 0.9),
        ("50%_duplicates", 0.5),
        ("10%_duplicates_mostly_new", 0.1),
        ("0%_duplicates_pure_insert", 0.0),
    ];

    // Проверяем корректность до запуска измерений.
    sanity_check(COUNT, 0.9);
    sanity_check(COUNT, 0.0);

    let mut group = c.benchmark_group("Proxy_Merge");
    group.throughput(Throughput::Elements(COUNT as u64));

    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(COUNT, overlap);

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

        group.bench_with_input(
            BenchmarkId::new("hashmap_raw_entry", name),
            &(),
            |b, _| {
                b.iter_batched(
                    || (old.clone(), new.clone()),
                    |(o, n)| black_box(merge_hashmap_raw_entry(o, n)),
                    BatchSize::LargeInput,
                );
            },
        );

        group.bench_with_input(BenchmarkId::new("std_entry", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_std_entry(o, n)),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, bench_proxy_merge);
criterion_main!(benches);
