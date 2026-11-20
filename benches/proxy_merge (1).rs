#[path = "../src/structer.rs"]
mod structer;

use structer::Proxy;

use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use hashbrown::hash_map::Entry as HbEntry;
use hashbrown::{HashMap as HbHashMap, HashTable, HashSet as HbHashSet};
use std::collections::hash_map::Entry as StdEntry;
use std::collections::HashMap as StdHashMap;
use std::hash::{BuildHasher, Hash, Hasher};

// ВАЖНО: Proxy должен реализовывать Clone, а Hash/Eq — только по (ip, port).
// Если Hash/Eq выведены derive по всем полям (включая last_checked),
// слияние в каждом варианте сработает неправильно — sanity_check() это поймает.
//
// Требования к зависимостям (Cargo.toml):
//   hashbrown = "0.17"   # без feature "raw": она удалена с версии 0.15
//   criterion = "0.5"
// MSRV для hashbrown 0.17 — Rust 1.85.

/// Считает хеш Proxy с учётом только (ip, port) — потому и нужен ручной Hash.
fn proxy_hash<H: BuildHasher>(p: &Proxy, s: &H) -> u64 {
    let mut h = s.build_hasher();
    p.hash(&mut h);
    h.finish()
}

// ----------------------------------------------------------------------------
// Вариант 1: hashbrown::HashSet через take + insert (2 поиска по хешу)
// ----------------------------------------------------------------------------
fn merge_hashset_take_insert(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut proxy_set: HbHashSet<Proxy> = old_proxies.into_iter().collect();

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

// ----------------------------------------------------------------------------
// Вариант 2: hashbrown::HashTable через find_mut + insert_unique.
// Замена удалённого raw_entry_mut: один поиск по хешу, мутация записи на месте.
// ----------------------------------------------------------------------------
fn merge_hashtable_find_mut(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut table: HashTable<Proxy> =
        HashTable::with_capacity(old_proxies.len() + new_proxies.len());
    let hasher = table.hasher().clone();

    for proxy in old_proxies {
        let h = proxy_hash(&proxy, &hasher);
        table.insert_unique(h, proxy, |p| proxy_hash(p, &hasher));
    }

    for new_proxy in new_proxies {
        let h = proxy_hash(&new_proxy, &hasher);

        // Ветку вставки НЕ делаем else к if let: заимствование new_proxy в замыкании
        // живёт до конца if let, поэтому сначала исчерпываем ветку обновления.
        if let Some(existing) = table.find_mut(h, |p| p == &new_proxy) {
            if new_proxy.last_checked > existing.last_checked {
                existing.last_checked = new_proxy.last_checked;
            }
        }
        // Сюда попадаем только если записи не было: заимствование уже отпущено.
        table.insert_unique(h, new_proxy, |p| proxy_hash(p, &hasher));
    }

    table.into_iter().collect()
}

// ----------------------------------------------------------------------------
// Вариант 3: hashbrown::HashMap<(String, String), Proxy> через entry.
// Клон ключа в каждой итерации — цена за безопасный entry API.
// ----------------------------------------------------------------------------
fn merge_hb_map_entry(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HbHashMap<(String, String), Proxy> = old_proxies
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

// ----------------------------------------------------------------------------
// Вариант 4: std::HashMap через entry — базлайн со стандартным (SipHash) хешером.
// Отличие от варианта 3 изолирует вклад хешера foldhash, а не структуры таблицы.
// ----------------------------------------------------------------------------
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

const VARIANTS: [&str; 4] = [
    "hashset_take_insert",
    "hashtable_find_mut",
    "hb_map_entry",
    "std_map_entry",
];

fn run_variant(name: &str, old: Vec<Proxy>, new: Vec<Proxy>) -> Vec<Proxy> {
    match name {
        "hashset_take_insert" => merge_hashset_take_insert(old, new),
        "hashtable_find_mut" => merge_hashtable_find_mut(old, new),
        "hb_map_entry" => merge_hb_map_entry(old, new),
        "std_map_entry" => merge_std_map_entry(old, new),
        _ => unreachable!(),
    }
}

/// Ожидаемая длина результата: старые + только новые из новой выборки.
fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    let offset = ((1.0 - overlap_ratio) * count as f64) as usize;
    2 * count - offset
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

/// Проверка корректности всех вариантов ДО замеров.
/// При derive(Hash) по всем полям слияние даст дубликаты — тест упадёт здесь, а не в проде.
fn sanity_check(count: usize) {
    let scenarios = [0.9, 0.5, 0.1, 0.0];

    for overlap in scenarios {
        let (old, new) = generate_data(count, overlap);
        let expected_len = expected_merged_len(count, overlap);

        // Эталон: последний вариант (std_map_entry) как источник правды.
        let reference = merge_std_map_entry(old.clone(), new.clone());
        assert_eq!(reference.len(), expected_len, "эталон: неверная длина");

        let mut reference_map: StdHashMap<(String, String), String> = reference
            .into_iter()
            .map(|p| ((p.ip.clone(), p.port.clone()), p.last_checked))
            .collect();

        for variant in VARIANTS {
            let merged = run_variant(variant, old.clone(), new.clone());
            assert_eq!(
                merged.len(),
                expected_len,
                "{variant}: длина {}/{} при overlap {overlap}",
                merged.len(),
                expected_len
            );
            for p in merged {
                let lc = reference_map
                    .remove(&(p.ip.clone(), p.port.clone()))
                    .unwrap_or_else(|| panic!("{variant}: лишний прокси {}", p.ip));
                assert_eq!(lc, p.last_checked, "{variant}: не тот last_checked у {}", p.ip);
            }
            assert!(reference_map.is_empty(), "{variant}: потеряны записи");
        }
    }

    println!("sanity_check: все варианты корректны");
}

fn bench_proxy_merge(c: &mut Criterion) {
    sanity_check(50_000);

    let count = 50_000;
    let scenarios = [
        ("90%_duplicates", 0.9),
        ("50%_duplicates", 0.5),
        ("10%_duplicates_mostly_new", 0.1),
        ("0%_duplicates_pure_insert", 0.0),
    ];

    let mut group = c.benchmark_group("Proxy_Merge_50k");
    group.throughput(Throughput::Elements(count as u64));

    for (name, overlap) in scenarios {
        let (old, new) = generate_data(count, overlap);

        for variant in VARIANTS {
            group.bench_with_input(BenchmarkId::new(variant, name), &(), |b, _| {
                b.iter_batched(
                    || (old.clone(), new.clone()),
                    |(o, n)| black_box(run_variant(variant, o, n)),
                    BatchSize::LargeInput,
                );
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_proxy_merge);
criterion_main!(benches);
