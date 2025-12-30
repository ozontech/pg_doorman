use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use xxhash_rust::xxh3::Xxh3;
use zerocopy::IntoBytes;

#[derive(Clone)]
struct Parse {
    pub query: String,
    pub num_params: u16,
    pub param_types: Vec<u32>,
}

impl Parse {
    fn hashed_size(&self) -> usize {
        self.query.len()
            + std::mem::size_of::<u16>()
            + self.param_types.len() * std::mem::size_of::<u32>()
    }
}

fn build_canonical_bytes(data: &Parse) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.hashed_size());

    out.extend_from_slice(data.query.as_bytes());
    out.extend_from_slice(&data.num_params.to_ne_bytes());
    out.extend_from_slice(data.param_types.as_slice().as_bytes());

    debug_assert_eq!(out.len(), data.hashed_size());
    out
}

fn xxhash3_structured(data: &Parse) -> u64 {
    let mut h = Xxh3::default();

    h.write(data.query.as_bytes());
    h.write_u16(data.num_params);
    h.write(data.param_types.as_slice().as_bytes());

    h.finish()
}

fn default_hasher_structured(data: &Parse) -> u64 {
    let mut h = DefaultHasher::default();

    h.write(data.query.as_bytes());
    h.write_u16(data.num_params);
    h.write(data.param_types.as_slice().as_bytes());

    h.finish()
}

fn default_hasher_alloc_format(data: &Parse) -> u64 {
    let mut h = DefaultHasher::new();

    let concatenated = format!(
        "{}{}{}",
        data.query,
        data.num_params,
        data.param_types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );

    h.write(concatenated.as_bytes());
    h.finish()
}

fn create_test_data() -> Vec<(&'static str, Parse)> {
    vec![
        (
            "small",
            Parse {
                query: "SELECT * FROM t WHERE id = $1".to_string(),
                num_params: 1,
                param_types: vec![23],
            },
        ),
        (
            "medium",
            Parse {
                query: "SELECT t1.value, t2.value FROM table_1 t1 INNER JOIN table_2 t2 ON t1.id = t2.id WHERE t.id IN ($1, $2)".to_string(),
                num_params: 2,
                param_types: vec![23, 23],
            },
        ),
        (
            "large",
            Parse {
                query: "SELECT t1.col1, t1.col2, t1.col3, t2.col1, t2.col2, t3.col1 FROM table_1 t1 INNER JOIN table_2 t2 ON t1.id = t2.id LEFT JOIN table_3 t3 ON t2.id = t3.id WHERE t1.status = $1 AND t2.created_at > $2 AND t3.updated_at < $3 ORDER BY t1.created_at DESC LIMIT $4 OFFSET $5".to_string(),
                num_params: 5,
                param_types: vec![25, 1114, 1114, 23, 23],
            },
        ),
        (
            "extra_large",
            Parse {
                query: format!(
                    "SELECT * FROM table WHERE id IN ({})",
                    (0..100)
                        .map(|i| format!("${}", i + 1))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                num_params: 100,
                param_types: (0..100).map(|_| 23).collect(),
            },
        ),
    ]
}

fn hash_benchmark_comparison(c: &mut Criterion) {
    for (name, parse) in create_test_data() {
        let bytes = build_canonical_bytes(&parse);

        let mut group = c.benchmark_group(format!("hash/{name}"));
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.sample_size(100);
        group.measurement_time(std::time::Duration::from_secs(10));

        group.bench_function("xxhash3_structured", |b| {
            b.iter(|| xxhash3_structured(std::hint::black_box(&parse)))
        });
        group.bench_function("default_hasher_structured", |b| {
            b.iter(|| default_hasher_structured(std::hint::black_box(&parse)))
        });
        group.bench_function("default_hasher_alloc_format", |b| {
            b.iter(|| default_hasher_alloc_format(std::hint::black_box(&parse)))
        });

        group.finish();
    }
}

criterion_group!(benches, hash_benchmark_comparison);
criterion_main!(benches);
