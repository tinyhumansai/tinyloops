fn main() {
    let t = tinyloops::Thresholds::default();
    let mut s = tinyloops::LoopState::new("g");
    let start = std::time::Instant::now();
    for i in 0..2000u32 {
        s.attempts = i % 10;
        let r = tinyloops::evaluate_ladder(&s, "loop", &t).unwrap();
        std::hint::black_box(r);
    }
    println!("2000 evals in {:?}", start.elapsed());
}
