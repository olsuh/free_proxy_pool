//! Бенчмарки слияния списков прокси (criterion 0.8 + hashbrown 0.17).
//!
//! Требования к `Proxy`:
//! - `Clone` + `Debug`;
//! - ручные реализации `Hash` и `PartialEq` ТОЛЬКО по (ip, port):
//!
//! ```ignore
//! impl Hash for Proxy {
//!     fn hash<H: Hasher>(&self, state: &mut H) {
//!         self.ip.hash(state);
//!         self.port.hash(state);
//!     }
//! }
//! impl PartialEq for Proxy {
//!     fn eq(&self, other: &Self) -> bool {
//!         self.ip == other.ip && self.port == other.port
//!     }
//! }
//! impl Eq for Proxy {}
//! ```
//!
//! Cargo.toml:
//!
//! ```toml
//! [dev-dependencies]
//! criterion = "0.8.2"
//! hashbrown = "0.17"
//! ```

#[path = "../src/structer.rs"]
#[allow(dead_code)] // тянем весь structer.rs, не всё в нём нужно бенчу
mod structer;

use structer::Proxy;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use hashbrown::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::collections::HashMap as StdHashMap;
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::hint::black_box;

const BENCH_COUNT: usize = 50_000;

const SCENARIOS: [(&str, f64); 4] = [
    ("90%_duplicates", 0.9),
    ("50%_duplicates", 0.5),
    ("10%_duplicates_mostly_new", 0.1),
    ("0%_duplicates_pure_insert", 0.0),
];

/// Смещение диапазона новых прокси. Считается ОДИН раз и используется
/// и генерацией, и формулой ожидаемой длины — так они не могут разойтись.
/// Округление обязательно: 1.0 - 0.9 = 0.09999999999999998 в f64,
/// и усечение `as usize` давало бы 4999 вместо 5000.
fn overlap_offset(count: usize, overlap_ratio: f64) -> usize {
    (count as f64 * (1.0 - overlap_ratio)).round() as usize
}

/// Ожидаемый размер результата: old + new - дубликаты.
fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    let offset = overlap_offset(count, overlap_ratio).min(count);
    let duplicates = count.saturating_sub(offset);
    2 * count - duplicates
}

/// Хеш одного значения с фиксированным RandomState.
fn make_hash(build: &RandomState, val: &Proxy) -> u64 {
    let mut h = build.build_hasher();
    val.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Вариант 1: hashbrown::HashSet, take + insert (2 поиска по хешу)
// ---------------------------------------------------------------------------
fn merge_hashset_take_insert(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_set: HashSet<Proxy> = old_proxies.into_iter().collect();

    for new_proxy in new_proxies {
        if let Some(mut existing) = proxy_set.take(&new_proxy) {
            if new_proxy.last_checked > existing.last_checked {
                existing.last_checked = new_proxy.last_checked;
            }
            proxy_set.insert(existing);
        } else {
            proxy_set.insert(new_proxy);
        }
    }

    proxy_set.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Вариант 2: hashbrown::HashTable, 1 поиск по хешу + мутация на месте
// (наследник выпиленного в 0.15+ raw_entry_mut)
// ---------------------------------------------------------------------------
fn merge_hashtable_find_mut(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let hasher = RandomState::new();
    let mut table: hashbrown::HashTable<Proxy> = HashTable::with_capacity(old_proxies.len());

    for p in old_proxies {
        let hash = make_hash(&hasher, &p);
        table.insert_unique(hash, p, |p| make_hash(&hasher, p));
    }

    for new_proxy in new_proxies {
        let hash = make_hash(&hasher, &new_proxy);
        if let Some(existing) = table.find_mut(hash, |p| p == &new_proxy) {
            if new_proxy.last_checked > existing.last_checked {
                existing.last_checked = new_proxy.last_checked;
            }
        } else {
            table.insert_unique(hash, new_proxy, |p| make_hash(&hasher, p));
        }
    }

    table.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Вариант 3: hashbrown::HashMap<(String, String), Proxy> через entry (1 клон ключа)
// ---------------------------------------------------------------------------
fn merge_hb_map_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HashMap<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();

    for np in new_proxies {
        match map.entry((np.ip.clone(), np.port.clone())) {
            hashbrown::hash_map::Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if np.last_checked > existing.last_checked {
                    existing.last_checked = np.last_checked;
                }
            }
            hashbrown::hash_map::Entry::Vacant(e) => {
                e.insert(np);
            }
        }
    }

    map.into_values().collect()
}

// ---------------------------------------------------------------------------
// Вариант 4 (эталон): std::HashMap, штатный SipHash
// ---------------------------------------------------------------------------
fn merge_std_map_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: StdHashMap<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();

    for np in new_proxies {
        match map.entry((np.ip.clone(), np.port.clone())) {
            Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if np.last_checked > existing.last_checked {
                    existing.last_checked = np.last_checked;
                }
            }
            Entry::Vacant(e) => {
                e.insert(np);
            }
        }
    }

    map.into_values().collect()
}

// ---------------------------------------------------------------------------
// Генерация данных
// ---------------------------------------------------------------------------
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

    let offset = overlap_offset(count, overlap_ratio);
    let new: Vec<Proxy> = (offset..offset + count)
        .map(|i| make_proxy(i, "2026.11.20 01:00:00"))
        .collect();

    (old, new)
}

// ---------------------------------------------------------------------------
// Проверка корректности перед замерами
// ---------------------------------------------------------------------------
fn sanity_check(_c: &mut Criterion) {
    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(BENCH_COUNT, overlap);
        let expected = expected_merged_len(BENCH_COUNT, overlap);

        // Эталон: std::HashMap через entry
        let reference = merge_std_map_entry(old.clone(), new.clone());
        assert_eq!(
            reference.len(),
            expected,
            "std_map_entry: {name}: expected {expected}, got {}",
            reference.len()
        );

        let ref_map: StdHashMap<(String, String), String> = reference
            .iter()
            .map(|p| ((p.ip.clone(), p.port.clone()), p.last_checked.clone()))
            .collect();

        for (fname, merged) in [
            (
                "hashset_take_insert",
                merge_hashset_take_insert(old.clone(), new.clone()),
            ),
            (
                "hashtable_find_mut",
                merge_hashtable_find_mut(old.clone(), new.clone()),
            ),
            (
                "hb_map_entry",
                merge_hb_map_entry(old.clone(), new.clone()),
            ),
        ] {
            assert_eq!(
                merged.len(),
                expected,
                "{fname}: {name}: expected {expected}, got {}",
                merged.len()
            );
            for p in &merged {
                assert_eq!(
                    ref_map.get(&(p.ip.clone(), p.port.clone())).map(|s| s.as_str()),
                    Some(p.last_checked.as_str()),
                    "{fname}: {name}: mismatch at {}:{}",
                    p.ip,
                    p.port
                );
            }
        }

        println!("sanity ok: {name} => merged {expected} из {}", 2 * BENCH_COUNT);
    }
}

// ---------------------------------------------------------------------------
// Замеры
// ---------------------------------------------------------------------------
fn bench_proxy_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proxy_Merge_50k");
    group.throughput(Throughput::Elements(BENCH_COUNT as u64));

    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(BENCH_COUNT, overlap);

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
            BenchmarkId::new("hashtable_find_mut", name),
            &(),
            |b, _| {
                b.iter_batched(
                    || (old.clone(), new.clone()),
                    |(o, n)| black_box(merge_hashtable_find_mut(o, n)),
                    BatchSize::LargeInput,
                );
            },
        );

        group.bench_with_input(BenchmarkId::new("hb_map_entry", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_hb_map_entry(o, n)),
                BatchSize::LargeInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("std_map_entry", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_std_map_entry(o, n)),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, sanity_check, bench_proxy_merge);
criterion_main!(benches);
