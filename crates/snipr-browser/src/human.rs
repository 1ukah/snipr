//! Entrada com cara de humano: trajetórias de mouse em curva, tempos variáveis.

use rand::Rng;

pub fn rnd(lo: u64, hi: u64) -> u64 {
    rand::thread_rng().gen_range(lo..hi.max(lo + 1))
}

pub fn jitter(amount: f64) -> f64 {
    if amount <= 0.0 {
        return 0.0;
    }
    rand::thread_rng().gen_range(-amount..amount)
}

pub fn key_delay() -> u64 {
    let mut r = rand::thread_rng();
    // Pausas ocasionais mais longas, como alguém digitando de verdade.
    if r.gen_bool(0.08) {
        r.gen_range(180..380)
    } else {
        r.gen_range(45..140)
    }
}

/// Curva de Bézier cúbica com pontos de controle aleatórios e easing.
/// Retorna (x, y, atraso_ms) para cada passo.
pub fn mouse_path(from: (f64, f64), to: (f64, f64)) -> Vec<(f64, f64, u64)> {
    let mut r = rand::thread_rng();
    let dist = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
    let steps = ((dist / 25.0) as usize).clamp(8, 32);
    let spread = (dist * 0.25).clamp(10.0, 160.0);
    let c1 = (
        from.0 + (to.0 - from.0) * 0.3 + r.gen_range(-spread..spread),
        from.1 + (to.1 - from.1) * 0.3 + r.gen_range(-spread..spread),
    );
    let c2 = (
        from.0 + (to.0 - from.0) * 0.7 + r.gen_range(-spread..spread),
        from.1 + (to.1 - from.1) * 0.7 + r.gen_range(-spread..spread),
    );
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            let t = t * t * (3.0 - 2.0 * t); // ease in-out
            let u = 1.0 - t;
            let x = u.powi(3) * from.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t.powi(3) * to.0;
            let y = u.powi(3) * from.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t.powi(3) * to.1;
            (x.max(0.0), y.max(0.0), r.gen_range(5..16))
        })
        .collect()
}
