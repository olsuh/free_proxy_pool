// benches/proxy_merge.rs
// Бенчмарк слияния прокси: сравнение хешеров (foldhash по умолчанию vs FxHash)
// на hashbrown и std, а также мапы vs множества.
//
// Cargo.toml:
//   [dev-dependencies]
//   criterion = "0.8.2"
//   hashbrown = "0.17"
//   rustc-hash = "2.1"   # FxBuildHasher появился в 2.x, в 1.x его нет
//
// ВАЖНО: Proxy должен реализовывать Hash и Eq только по (ip, port):
//
//   impl Hash for Proxy {
//       fn hash<H: Hasher>(&self, state: &mut H) {
//           self.ip.hash(state);
//           self.port.hash(state);
//       }
//   }
//   impl PartialEq for Proxy {
//       fn eq(&self, other: &Self) -> bool {
//           self.ip == other.ip && self.port == other.port
//       }
//   }
//   impl Eq for Proxy {}

#[path = "../src/structer.rs"]
#[allow(dead_code)]
mod structer;

use structer::Proxy;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use hashbrown::{HashMap as HbHashMap, HashSet as HbHashSet};
use rustc_hash::FxBuildHasher;
use std::collections::hash_map::Entry as StdEntry;
use std::collections::{HashMap as StdHashMap, HashSet as StdHashSet};
use std::hint::black_box;
use std::time::Duration;

// ==== Типы для сравнения хешеров и структур ====

// hashbrown с дефолтным foldhash
type HbSetFold = HbHashSet<Proxy>;
// hashbrown с FxHasher
type HbSetFx = HbHashSet<Proxy, FxBuildHasher>;
// std с FxHasher
type StdSetFx = StdHashSet<Proxy, FxBuildHasher>;
// мапы с FxHasher (клонирование ключей (ip, port))
type HbMapFx = HbHashMap<(String, String), Proxy, FxBuildHasher>;
type StdMapFx = StdHashMap<(String, String), Proxy, FxBuildHasher>;

const COUNT: usize = 50_000;
const SCENARIOS: &[(&str, f64)] = &[
    ("90%_duplicates", 0.9),
    ("50%_duplicates", 0.5),
    ("10%_duplicates_mostly_new", 0.1),
    ("0%_duplicates_pure_insert", 0.0),
];

// ==== Единая точка расчёта смещения (исправленная проблема с f64) ====

fn overlap_offset(count: usize, overlap_ratio: f64) -> usize {
    (count as f64 * (1.0 - overlap_ratio)).round() as usize
}

fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    2 * count - (count - overlap_offset(count, overlap_ratio))
}

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

// ==== Варианты слияния ====

// 1. hashbrown::HashSet, дефолтный foldhash, take + insert
fn merge_hb_set_fold(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut set: HbSetFold = old_proxies.into_iter().collect();
    for new_proxy in new_proxies {
        if let Some(mut existing) = set.take(&new_proxy) {
            if new_proxy.last_checked > existing.last_checked {
                existing.last_checked = new_proxy.last_checked;
            }
            set.insert(existing);
        } else {
            set.insert(new_proxy);
        }
    }
    set.into_iter().collect()
}

// 2. hashbrown::HashSet, FxHasher
fn merge_hb_set_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut set: HbSetFx = old_proxies.into_iter().collect();
    for new_proxy in new_proxies {
        if let Some(mut existing) = set.take(&new_proxy) {
            if new_proxy.last_checked > existing.last_checked {
                existing.last_checked = new_proxy.last_checked;
            }
            set.insert(existing);
        } else {
            set.insert(new_proxy);
        }
    }
    set.into_iter().collect()
}

// 3. std::HashSet, FxHasher (можно и get_mut, но для честного сравнения — тот же код)
fn merge_std_set_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut set: StdSetFx = old_proxies.into_iter().collect();
    for new_proxy in new_proxies {
        if let Some(mut existing) = set.take(&new_proxy) {
            if new_proxy.last_checked > existing.last_checked {
                existing.last_checked = new_proxy.last_checked;
            }
            set.insert(existing);
        } else {
            set.insert(new_proxy);
        }
    }
    set.into_iter().collect()
}

// 4. hashbrown::HashMap<(String, String), Proxy>, FxHasher, entry
fn merge_hb_map_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HbMapFx = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();
    for np in new_proxies {
        match map.entry((np.ip.clone(), np.port.clone())) {
            hashbrown::hash_map::Entry::Occupied(mut e) => {
                let ex = e.get_mut();
                if np.last_checked > ex.last_checked {
                    ex.last_checked = np.last_checked;
                }
            }
            hashbrown::hash_map::Entry::Vacant(e) => {
                e.insert(np);
            }
        }
    }
    map.into_values().collect()
}

// 5. std::HashMap, FxHasher, entry — эталон для sanity check
fn merge_std_map_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: StdMapFx = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();
    for np in new_proxies {
        match map.entry((np.ip.clone(), np.port.clone())) {
            StdEntry::Occupied(mut e) => {
                let ex = e.get_mut();
                if np.last_checked > ex.last_checked {
                    ex.last_checked = np.last_checked;
                }
            }
            StdEntry::Vacant(e) => {
                e.insert(np);
            }
        }
    }
    map.into_values().collect()
}

// ==== Проверка корректности до замеров ====

fn merge_variants() -> Vec<(&'static str, fn(Vec<Proxy>, Vec<Proxy>) -> Vec<Proxy>)> {
    vec![
        ("hb_set_fold", merge_hb_set_fold as fn(_, _) -> _),
        ("hb_set_fx", merge_hb_set_fx as fn(_, _) -> _),
        ("std_set_fx", merge_std_set_fx as fn(_, _) -> _),
        ("hb_map_fx", merge_hb_map_fx as fn(_, _) -> _),
        ("std_map_fx", merge_std_map_fx as fn(_, _) -> _),
    ]
}

fn sanity_check() {
    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(COUNT, *overlap);
        let expected = expected_merged_len(COUNT, *overlap);

        // Эталон: std мапа с Fx
        let reference = merge_std_map_fx(old.clone(), new.clone());
        assert_eq!(
            reference.len(),
            expected,
            "std_map_fx: {name}: ожидалось {expected}, получено {}",
            reference.len()
        );

        // Сверяем все варианты и по длине, и по содержимому last_checked
        let reference_state: StdMapFx = reference
            .into_iter()
            .map(|p| ((p.ip.clone(), p.port.clone()), p.last_checked.clone()))
            .collect();

        for (fn_name, f) in merge_variants() {
            let merged = f(old.clone(), new.clone());
            assert_eq!(
                merged.len(),
                expected,
                "{fn_name}: {name}: ожидалось {expected}, получено {}",
                merged.len()
            );
            let state: StdMapFx = merged
                .into_iter()
                .map(|p| ((p.ip, p.port), p.last_checked))
                .collect();
            assert_eq!(
                state, reference_state,
                "{fn_name}: {name}: состояние last_checked не совпало с эталоном"
            );
        }

        println!("sanity ok: {name} => merged {expected} из {}", 2 * COUNT);
    }
}

// ==== Сам бенчмарк ====

fn bench_proxy_merge(c: &mut Criterion) {
    sanity_check();

    let mut group = c.benchmark_group("Proxy_Merge_50k");
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(3));
    group.measurement_time(Duration::from_secs(15));
    group.throughput(Throughput::Elements((2 * COUNT) as u64));

    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(COUNT, *overlap);

        for (fn_name, f) in merge_variants() {
            group.bench_with_input(
                BenchmarkId::new(fn_name, name),
                &(),
                |b, _| {
                    b.iter_batched(
                        || (old.clone(), new.clone()),
                        |(o, n)| black_box(f(o, n)),
                        BatchSize::LargeInput,
                    );
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_proxy_merge);
criterion_main!(benches);
