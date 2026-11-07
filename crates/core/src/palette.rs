use std::collections::HashMap;

use image::RgbaImage;
use rayon::prelude::*;

use crate::color::{Color, key};
use crate::parallel;

#[derive(Clone, Copy)]
pub struct Entry {
    pub color: Color,
    pub count: u32,
}

/// Слияние двух гистограмм: суммы точные, поэтому порядок слияния не важен.
fn merge(mut a: HashMap<u32, u32>, b: HashMap<u32, u32>) -> HashMap<u32, u32> {
    // Меньшую карту вливаем в большую: меньше реаллокаций.
    if a.len() < b.len() {
        let mut large = b;
        for (k, v) in a {
            *large.entry(k).or_insert(0) += v;
        }
        large
    } else {
        for (k, v) in b {
            *a.entry(k).or_insert(0) += v;
        }
        a
    }
}

fn count_row(map: &mut HashMap<u32, u32>, row: &[[u8; 4]]) {
    for p in row {
        if p[3] == 0 {
            continue; // прозрачные пиксели в палитру не попадают
        }
        *map.entry(key([p[0], p[1], p[2]])).or_insert(0) += 1;
    }
}

/// Уникальные цвета непрозрачных пикселей с количеством пикселей каждого цвета.
///
/// Гистограмма считается по строкам: каждый поток собирает свою карту, затем
/// карты сливаются (суммы целые, порядок слияния на результат не влияет) и
/// сортируются по ключу цвета. Поэтому порядок в `HashMap` не виден наружу.
pub fn histogram(img: &RgbaImage) -> Vec<Entry> {
    let (w, h) = img.dimensions();
    let raw = img.as_raw().as_chunks::<4>().0;

    let map = if w > 0 && h > 0 && parallel::should_parallel(raw.len()) {
        raw.chunks(w as usize)
            .collect::<Vec<_>>()
            .par_iter()
            .fold(HashMap::new, |mut acc, row| {
                count_row(&mut acc, row);
                acc
            })
            .reduce(HashMap::new, merge)
    } else {
        let mut map = HashMap::new();
        count_row(&mut map, raw);
        map
    };

    let mut entries: Vec<Entry> = map
        .into_iter()
        .map(|(k, count)| Entry {
            color: [(k >> 16) as u8, (k >> 8) as u8, k as u8],
            count,
        })
        .collect();
    // HashMap даёт случайный порядок; сортируем ради воспроизводимого результата.
    entries.sort_unstable_by_key(|e| key(e.color));
    entries
}


/// Суммарное число пикселей коробки и взвешенная дисперсия (SSE) по каждому каналу:
/// сумма count * (значение - среднее)^2. Учитывает и площадь цвета, и его удалённость.
fn weighted_variance(entries: &[Entry]) -> (u64, [f64; 3]) {
    let total: u64 = entries.iter().map(|e| e.count as u64).sum();
    let mut mean = [0f64; 3];
    for e in entries {
        for (m, color) in mean.iter_mut().zip(e.color) {
            *m += color as f64 * e.count as f64;
        }
    }
    for m in &mut mean {
        *m /= total as f64;
    }
    let mut var = [0f64; 3];
    for e in entries {
        for c in 0..3 {
            let d = e.color[c] as f64 - mean[c];
            var[c] += d * d * e.count as f64;
        }
    }
    (total, var)
}

/// Среднее цветов коробки с учётом количества пикселей.
fn weighted_average(entries: &[Entry]) -> Color {
    let total: u64 = entries.iter().map(|e| e.count as u64).sum();
    let mut sum = [0u64; 3];
    for e in entries {
        for (s, color) in sum.iter_mut().zip(e.color) {
            *s += color as u64 * e.count as u64;
        }
    }
    [
        ((sum[0] + total / 2) / total) as u8,
        ((sum[1] + total / 2) / total) as u8,
        ((sum[2] + total / 2) / total) as u8,
    ]
}

/// Палитра, построенная из гистограммы методом median cut.
pub struct Palette {
    colors: Vec<Color>,
}

impl Palette {
    /// Строит палитру не более чем из `max_colors` цветов.
    pub fn median_cut(entries: Vec<Entry>, max_colors: usize) -> Self {
        let mut boxes: Vec<Vec<Entry>> = vec![entries];

        while boxes.len() < max_colors {
            // Делим коробку с наибольшей суммарной ошибкой (SSE = площадь * разброс).
            // Выбор только по числу пикселей теряет мелкие яркие детали (бирюзовый
            // камень), выбор только по разбросу портит большие светлые области;
            // SSE учитывает и то и другое.
            let mut best: Option<(usize, f64)> = None;
            for (i, b) in boxes.iter().enumerate() {
                if b.len() < 2 {
                    continue; // из одного цвета разрез невозможен
                }
                let sse: f64 = weighted_variance(b).1.iter().sum();
                if best.is_none_or(|(_, v)| sse > v) {
                    best = Some((i, sse));
                }
            }
            let Some((i, _)) = best else { break }; // делить больше нечего

            let mut b = boxes.swap_remove(i);
            let (total, var) = weighted_variance(&b);
            // Канал разреза: с наибольшей дисперсией.
            let ch = (0..3)
                .max_by(|&x, &y| var[x].partial_cmp(&var[y]).unwrap())
                .unwrap();
            b.sort_unstable_by_key(|e| e.color[ch]);

            // Разрез по взвешенной медиане: в обеих половинах примерно поровну пикселей.
            let mut acc = 0u64;
            let mut split = b.len() / 2;
            for (idx, e) in b.iter().enumerate() {
                acc += e.count as u64;
                if acc * 2 >= total {
                    split = idx + 1;
                    break;
                }
            }
            let split = split.clamp(1, b.len() - 1);

            let right = b.split_off(split);
            boxes.push(b);
            boxes.push(right);
        }

        Self {
            colors: boxes.iter().map(|b| weighted_average(b)).collect(),
        }
    }

    /// Число цветов в палитре.
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// Пуста ли палитра.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Ближайший цвет палитры к `p` (по квадрату евклидова расстояния).
    pub fn nearest(&self, p: Color) -> Color {
        let mut best = self.colors[0];
        let mut best_d = i32::MAX;
        for &c in &self.colors {
            let dr = c[0] as i32 - p[0] as i32;
            let dg = c[1] as i32 - p[1] as i32;
            let db = c[2] as i32 - p[2] as i32;
            let d = dr * dr + dg * dg + db * db;
            if d < best_d {
                best_d = d;
                best = c;
                if d == 0 {
                    break;
                }
            }
        }
        best
    }
}
