// benches/proxy_merge.rs
// Бенчмарк слияния прокси-данных.
// Требования: criterion = "0.8", hashbrown = "0.17" (без features), Rust 1.88+ (MSRV criterion 0.8).
//
// ВАЖНО: Proxy должен реализовывать Hash и Eq ТОЛЬКО по (ip, port).
// derive(Hash, PartialEq, Eq) по всем полям сломает слияние и sanity_check упадёт.

#[path = "../src/structer.rs"]
mod structer;

use structer::Proxy;

use std::collections::HashMap as StdHashMap;
use std::collections::hash_map::Entry as StdEntry;
use std::hash::{BuildHasher, Hash, Hasher};
use std::hint::black_box; // criterion::black_box устарел (с 0.6) — всегда std::hint::black_box

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use hashbrown::hash_map::Entry as HbEntry;
use hashbrown::{HashMap, HashTable, HashSet};

const COUNT: usize = 50_000;

// ---------- Вариант 1: hashbrown::HashSet, take + insert (два поиска по хешу) ----------

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

// ---------- Вариант 2: hashbrown::HashTable, find_mut (один поиск, мутация на месте) ----------
// Наследник снятого в hashbrown 0.15+ raw_entry_mut.

fn merge_hashtable_find_mut(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let hasher = std::collections::hash_map::RandomState::new();
    let mut table: HashTable<Proxy> = HashTable::with_capacity(old_proxies.len());

    // Заранее считаем хеш прокси — он нужен и для поиска, и для вставки.
    let hash_of = |p: &Proxy| -> u64 {
        let mut h = hasher.build_hasher();
        p.hash(&mut h);
        h.finish()
    };

    for p in old_proxies {
        let hash = hash_of(&p);
        table.insert_unique(hash, p, |p| hash_of(p));
    }

    for new_proxy in new_proxies {
        let hash = hash_of(&new_proxy);
        let ts = new_proxy.last_checked.clone();

        if let Some(existing) = table.find_mut(hash, |p| p == &new_proxy) {
            if ts > existing.last_checked {
                existing.last_checked = ts;
            }
        } else {
            // Ветка else — только после того, как find_mut вернула None и
            // заимствование new_proxy замыканием уже завершилось.
            table.insert_unique(hash, new_proxy, |p| hash_of(p));
        }
    }

    table.into_iter().collect()
}

// ---------- Вариант 3: hashbrown::HashMap<(String, String), Proxy> через entry ----------

fn merge_hb_map_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HashMap<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();

    for new_proxy in new_proxies {
        match map.entry((new_proxy.ip.clone(), new_proxy.port.clone())) {
            HbEntry::Occupied(mut e) => {
                let existing = e.get_mut();
                if new_proxy.last_checked > existing.last_checked {
                    existing.last_checked = new_proxy.last_checked;
                }
            }
            HbEntry::Vacant(e) => {
                e.insert(new_proxy);
            }
        }
    }

    map.into_values().collect()
}

// ---------- Вариант 4 (базлайн): std::collections::HashMap через entry ----------
// Медленнее по таблице и хешеру (SipHash против foldhash) — эталон простоты и корректности.

fn merge_std_map_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: StdHashMap<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();

    for new_proxy in new_proxies {
        match map.entry((new_proxy.ip.clone(), new_proxy.port.clone())) {
            StdEntry::Occupied(mut e) => {
                let existing = e.get_mut();
                if new_proxy.last_checked > existing.last_checked {
                    existing.last_checked = new_proxy.last_checked;
                }
            }
            StdEntry::Vacant(e) => {
                e.insert(new_proxy);
            }
        }
    }

    map.into_values().collect()
}

// ---------- Генерация данных ----------

fn make_proxy(i: usize, date: &str) -> Proxy {
    Proxy {
        ip: format!("192.168.{}.{}", (i / 255) % 255, i % 255),
        port: (8000 + (i % 1000)).to_string(),
        proxy_type: "http".to_string(),
        country: "UA".to_string(),
        last_checked: date.to_string(),
        check_number: 1,
        live_number: 1,
        live: true,
    }
}

fn generate_data(count: usize, overlap_ratio: f64) -> (Vec<Proxy>, Vec<Proxy>) {
    let old: Vec<Proxy> = (0..count)
        .map(|i| make_proxy(i, "2026.11.19 10:00:00"))
        .collect();

    let offset = ((1.0 - overlap_ratio) * count as f64) as usize;
    let new: Vec<Proxy> = (offset..offset + count)
        .map(|i| make_proxy(i, "2026.11.20 01:00:00"))
        .collect();

    (old, new)
}

// Ожидаемый размер результата: все old + не пересекающиеся с old элементы new.
fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    let duplicated = (overlap_ratio * count as f64) as usize;
    count + (count - duplicated)
}

// ---------- Проверка корректности перед замерами ----------

// Свернуть результат в (ip, port) -> last_checked.
fn as_map(proxies: &[Proxy]) -> StdHashMap<(&str, &str), &str> {
    proxies
        .iter()
        .map(|p| (p.ip.as_str(), p.port.as_str(), p.last_checked.as_str()))
        .map(|(ip, port, ts)| ((ip, port), ts))
        .collect()
}

fn sanity_check(_c: &mut Criterion) {
    let scenarios = [
        ("90%", 0.9),
        ("50%", 0.5),
        ("10%", 0.1),
        ("0%", 0.0),
    ];

    for (name, overlap) in scenarios {
        let (old, new) = generate_data(COUNT, overlap);
        let expected = expected_merged_len(COUNT, overlap);

        let reference = merge_std_map_entry(old.clone(), new.clone());
        assert_eq!(reference.len(), expected, "std_map_entry: {name}");
        let reference_map = as_map(&reference);

        let variants = [
            ("hashset_take_insert", merge_hashset_take_insert(old.clone(), new.clone())),
            ("hashtable_find_mut", merge_hashtable_find_mut(old.clone(), new.clone())),
            ("hb_map_entry", merge_hb_map_entry(old.clone(), new.clone())),
        ];

        for (fname, result) in variants {
            assert_eq!(result.len(), expected, "{fname}: {name} (len)");
            let result_map = as_map(&result);
            assert_eq!(
                result_map.len(),
                reference_map.len(),
                "{fname}: {name} (unique keys)"
            );
            for (key, ts) in &reference_map {
                assert_eq!(
                    result_map.get(key).copied(),
                    Some(*ts),
                    "{fname}: {name} ({key:?})"
                );
            }
        }
    }
}

// ---------- Бенчмарк ----------

fn bench_proxy_merge(c: &mut Criterion) {
    let scenarios = [
        ("90%_duplicates", 0.9),
        ("50%_duplicates", 0.5),
        ("10%_duplicates_mostly_new", 0.1),
        ("0%_duplicates_pure_insert", 0.0),
    ];

    let mut group = c.benchmark_group("Proxy_Merge_50k");
    group.throughput(Throughput::Elements(COUNT as u64));

    for (name, overlap) in scenarios {
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
