use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::future::Future;
use std::sync::Arc;

use pg_doorman::auth::auth_query::{AuthQueryCache, PasswordFetcher};
use pg_doorman::config::{AuthQueryConfig, Duration};
use pg_doorman::errors::Error;

/// Instant-return fetcher for cache hit benchmarks.
/// Never actually called on cache hits — exists only to satisfy the type system.
struct NoopFetcher;

impl PasswordFetcher for NoopFetcher {
    fn fetch<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = Result<Option<(String, String)>, Error>> + Send + 'a {
        async move {
            Ok(Some((
                username.to_string(),
                "md5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            )))
        }
    }
}

fn bench_config() -> AuthQueryConfig {
    AuthQueryConfig {
        query: String::new(),
        user: String::new(),
        password: String::new(),
        database: None,
        pool_size: 1,
        server_user: None,
        server_password: None,
        default_pool_size: 40,
        cache_ttl: Duration::from_hours(1),
        cache_failure_ttl: Duration::from_secs(30),
        min_interval: Duration::from_secs(1),
    }
}

fn cache_hit_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("auth_query_cache");
    group.throughput(Throughput::Elements(1));
    group.sample_size(500);
    group.measurement_time(std::time::Duration::from_secs(10));

    // -- Single-user cache hit (best case: same DashMap shard every time) --
    {
        let fetcher = Arc::new(NoopFetcher);
        let cache = AuthQueryCache::new(fetcher, &bench_config());
        rt.block_on(cache.get_or_fetch("test_user")).unwrap();

        group.bench_function("hit/single_user", |b| {
            b.iter(|| {
                rt.block_on(cache.get_or_fetch(std::hint::black_box("test_user")))
                    .unwrap()
            })
        });
    }

    // -- Multi-user cache hit (spreads across DashMap shards) --
    for &user_count in &[10, 100, 1000] {
        let fetcher = Arc::new(NoopFetcher);
        let cache = AuthQueryCache::new(fetcher, &bench_config());

        let usernames: Vec<String> = (0..user_count).map(|i| format!("user_{i}")).collect();
        for name in &usernames {
            rt.block_on(cache.get_or_fetch(name)).unwrap();
        }

        group.bench_function(BenchmarkId::new("hit/multi_user", user_count), |b| {
            let mut idx = 0usize;
            b.iter(|| {
                let username = &usernames[idx % usernames.len()];
                idx += 1;
                rt.block_on(cache.get_or_fetch(std::hint::black_box(username)))
                    .unwrap()
            })
        });
    }

    // -- Negative cache hit (user not found, cached as negative entry) --
    {
        let fetcher = Arc::new(NoopFetcher);
        // Override fetcher to return None for negative cache test
        let config = bench_config();
        let cache = AuthQueryCache::new(fetcher, &config);

        // Manually trigger a cache miss that returns Some, then invalidate doesn't help.
        // Instead, use a fetcher that returns None for the negative user.
        struct NegativeFetcher;
        impl PasswordFetcher for NegativeFetcher {
            fn fetch<'a>(
                &'a self,
                _username: &'a str,
            ) -> impl Future<Output = Result<Option<(String, String)>, Error>> + Send + 'a
            {
                async { Ok(None) }
            }
        }

        let neg_fetcher = Arc::new(NegativeFetcher);
        let neg_cache = AuthQueryCache::new(neg_fetcher, &config);
        rt.block_on(neg_cache.get_or_fetch("nonexistent")).unwrap();

        group.bench_function("hit/negative", |b| {
            b.iter(|| {
                rt.block_on(neg_cache.get_or_fetch(std::hint::black_box("nonexistent")))
                    .unwrap()
            })
        });

        // Drop the unused positive cache
        drop(cache);
    }

    group.finish();
}

criterion_group!(benches, cache_hit_benchmark);
criterion_main!(benches);
