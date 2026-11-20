#[path = "../src/structer.rs"]
#[allow(dead_code)]
mod structer;

use structer::Proxy;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use rustc_hash::FxBuildHasher;
use std::collections::{Entry as StdEntry, HashMap as StdHashMap, HashSet as StdHashSet};
use std::time::Duration;

// Пять вариантов: (структура) x (хешер)
type StdSetFx<T> = StdHashSet<T, FxBuildHasher>;
type StdMapFx<K, V> = StdHashMap<K, V, FxBuildHasher>;
type HbSetFold<T> = hashbrown::HashSet<T>; // дефолтный foldhash
type HbSetFx<T> = hashbrown::HashSet<T, FxBuildHasher>;
type HbMapFx<K, V> = hashbrown::HashMap<K, V, FxBuildHasher>;

// Требования к Proxy: Clone + Hash/Eq строго по (ip, port).
// Если Hash/Eq выведены derive по всем полям — sanity check ниже упадёт.

fn merge_hb_set_fold(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut set = HbSetFold::<Proxy>::default();
    set.extend(old_proxies);
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

fn merge_hb_set_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut set = HbSetFx::<Proxy>::default();
    set.extend(old_proxies);
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

fn merge_std_set_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut set = StdSetFx::<Proxy>::default();
    set.extend(old_proxies);
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

fn merge_hb_map_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: HbMapFx<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();
    for new_proxy in new_proxies {
        let key = (new_proxy.ip.clone(), new_proxy.port.clone());
        match map.entry(key) {
            hashbrown::hash_map::Entry::Occupied(mut e) => {
                if new_proxy.last_checked > e.get().last_checked {
                    e.get_mut().last_checked = new_proxy.last_checked;
                }
            }
            hashbrown::hash_map::Entry::Vacant(e) => {
                e.insert(new_proxy);
            }
        }
    }
    map.into_values().collect()
}

fn merge_std_map_fx(old_proxies: Vec<Proxy>, new_proxies: Vec<Proxy>) -> Vec<Proxy> {
    let mut map: StdMapFx<(String, String), Proxy> = old_proxies
        .into_iter()
        .map(|p| ((p.ip.clone(), p.port.clone()), p))
        .collect();
    for new_proxy in new_proxies {
        let key = (new_proxy.ip.clone(), new_proxy.port.clone());
        match map.entry(key) {
            StdEntry::Occupied(mut e) => {
                if new_proxy.last_checked > e.get().last_checked {
                    e.get_mut().last_checked = new_proxy.last_checked;
                }
            }
            StdEntry::Vacant(e) => {
                e.insert(new_proxy);
            }
        }
    }
    map.into_values().collect()
}

/// Смещение начала диапазона новых прокси: одинаковый расчёт для генерации и ожидания.
fn overlap_offset(count: usize, overlap_ratio: f64) -> usize {
    (count as f64 * (1.0 - overlap_ratio)).round() as usize
}

fn expected_merged_len(count: usize, overlap_ratio: f64) -> usize {
    // дубликатов в new: count - offset, итого merged = count + offset
    count + overlap_offset(count, overlap_ratio)
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

    let offset = overlap_offset(count, overlap_ratio);
    let new: Vec<Proxy> = (offset..offset + count)
        .map(|i| make_proxy(i, "2026.11.20 01:00:00"))
        .collect();

    (old, new)
}

/// Проверка корректности до замеров: все варианты обязаны совпасть с эталоном.
/// Эталон — std_map_fx (самая простая и проверенная семантика).
fn sanity_check(count: usize, scenarios: &[(&str, f64)]) {
    for (name, overlap) in scenarios {
        let (old, new) = generate_data(count, *overlap);

        let expected = expected_merged_len(count, *overlap);
        let reference = merge_std_map_fx(old.clone(), new.clone());
        assert_eq!(
            reference.len(),
            expected,
            "std_map_fx: {name}: len {} != {expected}",
            reference.len()
        );

        // справочная мапа (ip, port) -> last_checked
        let reference_map: StdMapFx<(String, String), String> = reference
            .into_iter()
            .map(|p| ((p.ip, p.port), p.last_checked))
            .collect();

        let variants: [(&str, fn(Vec<Proxy>, Vec<Proxy>) -> Vec<Proxy>); 4] = [
            ("hb_set_fold", merge_hb_set_fold as fn(_, _)),
            ("hb_set_fx", merge_hb_set_fx as fn(_, _)),
            ("std_set_fx", merge_std_set_fx as fn(_, _)),
            ("hb_map_fx", merge_hb_map_fx as fn(_, _)),
        ];

        for (variant_name, f) in variants {
            let merged = f(old.clone(), new.clone());
            assert_eq!(
                merged.len(),
                expected,
                "{variant_name}: {name}: len {} != {expected}",
                merged.len()
            );
            for p in merged {
                let ref_ts = reference_map
                    .get(&(p.ip.clone(), p.port.clone()))
                    .unwrap_or_else(|| panic!("{variant_name}: {name}: ip {} port {} не найден в эталоне", p.ip, p.port));
                assert_eq!(
                    &p.last_checked, ref_ts,
                    "{variant_name}: {name}: ({}:{}) last_checked {} != эталон {}",
                    p.ip, p.port, p.last_checked, ref_ts
                );
            }
        }

        println!("sanity ok: {name} => merged {expected} из {}", 2 * count);
    }
}

fn bench_proxy_merge(c: &mut Criterion) {
    const COUNT: usize = 50_000;
    let scenarios: [(&str, f64); 4] = [
        ("90%_duplicates", 0.9),
        ("50%_duplicates", 0.5),
        ("10%_duplicates_mostly_new", 0.1),
        ("0%_duplicates_pure_insert", 0.0),
    ];

    sanity_check(COUNT, &scenarios);

    let mut group = c.benchmark_group("Proxy_Merge_50k");
    group.throughput(Throughput::Elements(COUNT as u64));
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(5));
    group.measurement_time(Duration::from_secs(15));

    let variants: [(&str, fn(Vec<Proxy>, Vec<Proxy>) -> Vec<Proxy>); 5] = [
        ("hb_set_fold", merge_hb_set_fold as fn(_, _)),
        ("hb_set_fx", merge_hb_set_fx as fn(_, _)),
        ("std_set_fx", merge_std_set_fx as fn(_, _)),
        ("hb_map_fx", merge_hb_map_fx as fn(_, _)),
        ("std_map_fx", merge_std_map_fx as fn(_, _)),
    ];

    for (name, overlap) in scenarios {
        let (old, new) = generate_data(COUNT, overlap);

        for (variant_name, f) in variants {
            group.bench_with_input(
                BenchmarkId::new(variant_name, name),
                &(),
                |b, _| {
                    b.iter_batched(
                        || (old.clone(), new.clone()),
                        |(o, n)| std::hint::black_box(f(o, n)),
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
