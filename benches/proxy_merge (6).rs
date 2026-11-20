#[path = "../src/structer.rs"]
#[allow(dead_code)]
mod structer;

use structer::Proxy;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use hashbrown::{HashMap as HbHashMap, HashSet as HbHashSet};
use rustc_hash::FxBuildHasher;
use std::collections::hash_map::Entry;
use std::collections::{HashMap as StdHashMap, HashSet as StdHashSet};
use std::hint::black_box;
use std::time::Duration;

// ВАЖНО: Hash/Eq для Proxy должны учитывать только (ip, port).
// Если они выведены через derive по всем полям — sanity_check упадёт до замеров.

const COUNT: usize = 50_000;

/// Смещение начала "новых" прокси: единая точка расчёта для генерации и ожидания.
/// round() вместо as, чтобы не терять единицу на плавающей точке (1.0 - 0.9 != 0.1).
fn overlap_offset(count: usize, overlap_ratio: f64) -> usize {
    (count as f64 * (1.0 - overlap_ratio)).round() as usize
}

fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    count + overlap_offset(count, overlap_ratio)
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

// ---------- Вариант 1: hashbrown HashSet, foldhash (дефолт), take + insert ----------
fn merge_hb_set_fold(old: Vec<Proxy>, new: Vec<Proxy>) -> Vec<Proxy> {
    let mut set: HbHashSet<Proxy> = old.into_iter().collect();

    for n in new {
        if let Some(mut existing) = set.take(&n) {
            if n.last_checked > existing.last_checked {
                existing.last_checked = n.last_checked;
            }
            set.insert(existing);
        } else {
            set.insert(n);
        }
    }

    set.into_iter().collect()
}

// ---------- Вариант 2: hashbrown HashSet, FxHash, take + insert ----------
fn merge_hb_set_fx(old: Vec<Proxy>, new: Vec<Proxy>) -> Vec<Proxy> {
    let mut set: HbHashSet<Proxy, FxBuildHasher> = HbHashSet::with_hasher(FxBuildHasher);
    for p in old {
        set.insert(p);
    }

    for n in new {
        if let Some(mut existing) = set.take(&n) {
            if n.last_checked > existing.last_checked {
                existing.last_checked = n.last_checked;
            }
            set.insert(existing);
        } else {
            set.insert(n);
        }
    }

    set.into_iter().collect()
}

// ---------- Вариант 3: std HashSet, FxHash, take + insert ----------
fn merge_std_set_fx(old: Vec<Proxy>, new: Vec<Proxy>) -> Vec<Proxy> {
    let mut set: StdHashSet<Proxy, FxBuildHasher> = StdHashSet::with_hasher(FxBuildHasher);
    for p in old {
        set.insert(p);
    }

    for n in new {
        if let Some(mut existing) = set.take(&n) {
            if n.last_checked > existing.last_checked {
                existing.last_checked = n.last_checked;
            }
            set.insert(existing);
        } else {
            set.insert(n);
        }
    }

    set.into_iter().collect()
}

// ---------- Вариант 4: hashbrown HashMap<(String, String), Proxy>, FxHash, entry ----------
fn merge_hb_map_fx(old: Vec<Proxy>, new: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HbHashMap<(String, String), Proxy, FxBuildHasher> = HbHashMap::with_hasher(FxBuildHasher);
    for p in old {
        map.insert((p.ip.clone(), p.port.clone()), p);
    }

    for n in new {
        match map.entry((n.ip.clone(), n.port.clone())) {
            hashbrown::hash_map::Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if n.last_checked > existing.last_checked {
                    existing.last_checked = n.last_checked;
                }
            }
            hashbrown::hash_map::Entry::Vacant(e) => {
                e.insert(n);
            }
        }
    }

    map.into_values().collect()
}

// ---------- Вариант 5 (эталон): std HashMap<(String, String), Proxy>, FxHash, entry ----------
fn merge_std_map_fx(old: Vec<Proxy>, new: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: StdHashMap<(String, String), Proxy, FxBuildHasher> = StdHashMap::with_hasher(FxBuildHasher);
    for p in old {
        map.insert((p.ip.clone(), p.port.clone()), p);
    }

    for n in new {
        match map.entry((n.ip.clone(), n.port.clone())) {
            Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if n.last_checked > existing.last_checked {
                    existing.last_checked = n.last_checked;
                }
            }
            Entry::Vacant(e) => {
                e.insert(n);
            }
        }
    }

    map.into_values().collect()
}

const SCENARIOS: [(&str, f64); 4] = [
    ("90%_duplicates", 0.9),
    ("50%_duplicates", 0.5),
    ("10%_duplicates_mostly_new", 0.1),
    ("0%_duplicates_pure_insert", 0.0),
];

/// Сверка всех вариантов с эталоном (std_map_fx) до замеров.
/// Ловит дефектный Hash (derive по всем полям) и расхождения логики слияния.
fn sanity_check(_c: &mut Criterion) {
    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(COUNT, overlap);
        let expected = expected_merged_len(COUNT, overlap);

        let reference = merge_std_map_fx(old.clone(), new.clone());
        assert_eq!(reference.len(), expected, "std_map_fx: {name}");

        let ref_map: StdHashMap<(String, String), String, FxBuildHasher> = reference
            .into_iter()
            .map(|p| ((p.ip, p.port), p.last_checked))
            .collect();

        let merged_sets = [
            ("hb_set_fold", merge_hb_set_fold(old.clone(), new.clone())),
            ("hb_set_fx", merge_hb_set_fx(old.clone(), new.clone())),
            ("std_set_fx", merge_std_set_fx(old.clone(), new.clone())),
            ("hb_map_fx", merge_hb_map_fx(old.clone(), new.clone())),
        ];

        for (fname, merged) in merged_sets {
            assert_eq!(merged.len(), expected, "{fname}: {name}");
            let merged_map: StdHashMap<(String, String), String, FxBuildHasher> = merged
                .into_iter()
                .map(|p| ((p.ip, p.port), p.last_checked))
                .collect();
            assert_eq!(merged_map, ref_map, "{fname}: {name}");
        }

        println!("sanity ok: {name} => merged {expected} из {}", 2 * COUNT);
    }
}

fn bench_proxy_merge(c: &mut Criterion) {
    let mut group = c.benchmark_group("Proxy_Merge_50k");
    group.throughput(Throughput::Elements((COUNT * 2) as u64));
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(15));
    group.warm_up_time(Duration::from_secs(5));

    for (name, overlap) in SCENARIOS {
        let (old, new) = generate_data(COUNT, overlap);

        group.bench_with_input(BenchmarkId::new("hb_set_fold", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_hb_set_fold(o, n)),
                BatchSize::LargeInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("hb_set_fx", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_hb_set_fx(o, n)),
                BatchSize::LargeInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("std_set_fx", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_std_set_fx(o, n)),
                BatchSize::LargeInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("hb_map_fx", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_hb_map_fx(o, n)),
                BatchSize::LargeInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("std_map_fx", name), &(), |b, _| {
            b.iter_batched(
                || (old.clone(), new.clone()),
                |(o, n)| black_box(merge_std_map_fx(o, n)),
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, sanity_check, bench_proxy_merge);
criterion_main!(benches);
