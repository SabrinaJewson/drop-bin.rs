use std::collections::HashMap;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

type Heavy = HashMap<usize, Vec<usize>>;

fn make_heavy() -> Heavy {
    (0..1000).map(|v| (v, vec![v; 500])).collect()
}

fn drop_expensive(c: &mut Criterion) {
    c.bench_function("drop normal", |b| {
        b.iter_batched(make_heavy, drop, BatchSize::SmallInput)
    });

    c.bench_function("drop bin", |b| {
        let bin = drop_bin::Bin::new();

        b.iter_batched(make_heavy, |heavy| bin.add(heavy), BatchSize::LargeInput)
    });

    c.bench_function("drop thread", |b| {
        b.iter_batched(
            make_heavy,
            |heavy| {
                defer_drop::DeferDrop::new(heavy);
            },
            BatchSize::LargeInput,
        )
    });
}

criterion_group!(benches, drop_expensive);
criterion_main!(benches);
