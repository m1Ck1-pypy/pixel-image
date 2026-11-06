//! Очистка фона и мусора. Только стандартная библиотека, без внешних крейтов:
//! модуль можно собрать и проверить отдельно (`rustc --test crates/core/src/clean.rs`).
//!
//! Все функции работают с «сырыми» RGBA-буферами (4 байта на пиксель, строки подряд).

/// Самый частый цвет по периметру изображения (оценка цвета фона).
pub fn detect_background(rgba: &[u8], w: usize, h: usize) -> [u8; 3] {
    use std::collections::HashMap;
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    let mut add = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        *counts
            .entry([rgba[i], rgba[i + 1], rgba[i + 2]])
            .or_insert(0) += 1;
    };
    for x in 0..w {
        add(x, 0);
        add(x, h - 1);
    }
    for y in 1..h.saturating_sub(1) {
        add(0, y);
        add(w - 1, y);
    }
    // при равенстве берём меньший цвет, чтобы результат был детерминированным
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(c, _)| c)
        .unwrap_or([255, 255, 255])
}

/// Расстояние от пикселя до цвета фона: наибольшая разница по каналам.
#[inline]
fn dist(rgba: &[u8], i: usize, bg: [u8; 3]) -> u8 {
    let d = |a: u8, b: u8| a.abs_diff(b);
    d(rgba[i], bg[0])
        .max(d(rgba[i + 1], bg[1]))
        .max(d(rgba[i + 2], bg[2]))
}

/// Пиксель считается фоном, если он прозрачный или цвет в пределах `tol` от `bg`.
#[inline]
fn is_bg(rgba: &[u8], i: usize, bg: [u8; 3], tol: u8) -> bool {
    rgba[i + 3] == 0 || dist(rgba, i, bg) <= tol
}

/// Удаляет однотонный фон: заливка от границ изображения по пикселям, близким к
/// цвету фона (4-связность). Цвета внутри персонажа, не связанные с краем
/// (белые блики, кроссовки, замкнутые просветы), не затрагиваются.
///
/// Если `min_hole > 0`, дополнительно удаляются замкнутые просветы фона (между
/// ногами, между рукой и телом): связные области пикселей, близких к фону, до
/// которых заливка от краёв не дошла, но площадь которых не меньше `min_hole`
/// пикселей. Мелкие белые детали (блики, белки глаз) остаются.
///
/// После заливки пиксели вдоль новой границы, которые светлее остальных контура
/// (антиалиасинг, JPEG-ореол), снимаются слоями: `grow` слоёв по 4-соседству, если
/// расстояние до фона не больше `tol * 3`. Фон становится полностью прозрачным
/// (0, 0, 0, 0). Возвращает число сделанных прозрачными пикселей.
pub fn remove_background(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    bg: [u8; 3],
    tol: u8,
    grow: u32,
    min_hole: usize,
) -> usize {
    assert_eq!(rgba.len(), w * h * 4);
    let mut gone = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();

    // Затравки: все граничные пиксели, подходящие под фон.
    let seed = |x: usize, y: usize, gone: &mut Vec<bool>, stack: &mut Vec<usize>| {
        let p = y * w + x;
        if !gone[p] && is_bg(rgba, p * 4, bg, tol) {
            gone[p] = true;
            stack.push(p);
        }
    };
    for x in 0..w {
        seed(x, 0, &mut gone, &mut stack);
        seed(x, h - 1, &mut gone, &mut stack);
    }
    for y in 0..h {
        seed(0, y, &mut gone, &mut stack);
        seed(w - 1, y, &mut gone, &mut stack);
    }

    // Заливка
    while let Some(p) = stack.pop() {
        let (x, y) = (p % w, p / w);
        let mut visit = |q: usize| {
            if !gone[q] && is_bg(rgba, q * 4, bg, tol) {
                gone[q] = true;
                stack.push(q);
            }
        };
        if x > 0 {
            visit(p - 1);
        }
        if x + 1 < w {
            visit(p + 1);
        }
        if y > 0 {
            visit(p - w);
        }
        if y + 1 < h {
            visit(p + w);
        }
    }

    // Замкнутые просветы фона: крупные связные области цвета фона внутри объекта.
    if min_hole > 0 {
        let mut seen = vec![false; w * h];
        for start in 0..w * h {
            if gone[start] || seen[start] || !is_bg(rgba, start * 4, bg, tol) {
                continue;
            }
            let mut cells = vec![start];
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(p) = stack.pop() {
                let (x, y) = (p % w, p / w);
                let mut nb = [usize::MAX; 4];
                if x > 0 {
                    nb[0] = p - 1;
                }
                if x + 1 < w {
                    nb[1] = p + 1;
                }
                if y > 0 {
                    nb[2] = p - w;
                }
                if y + 1 < h {
                    nb[3] = p + w;
                }
                for q in nb {
                    if q != usize::MAX && !gone[q] && !seen[q] && is_bg(rgba, q * 4, bg, tol) {
                        seen[q] = true;
                        cells.push(q);
                        stack.push(q);
                    }
                }
            }
            if cells.len() >= min_hole {
                for q in cells {
                    gone[q] = true;
                }
            }
        }
    }

    // Снятие светлой каймы слоями вдоль найденной границы.
    let soft = tol.saturating_mul(3);
    for _ in 0..grow {
        let mut layer: Vec<usize> = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p = y * w + x;
                if gone[p] || dist(rgba, p * 4, bg) > soft {
                    continue;
                }
                let touches = (x > 0 && gone[p - 1])
                    || (x + 1 < w && gone[p + 1])
                    || (y > 0 && gone[p - w])
                    || (y + 1 < h && gone[p + w]);
                if touches {
                    layer.push(p);
                }
            }
        }
        if layer.is_empty() {
            break;
        }
        for p in layer {
            gone[p] = true;
        }
    }

    let mut removed = 0;
    for (p, g) in gone.iter().enumerate() {
        if *g {
            let i = p * 4;
            if rgba[i + 3] != 0 {
                removed += 1;
            }
            rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
        }
    }
    removed
}

/// Удаляет тонкие щели цвета фона у самой границы объекта. Заливка от краёв не
/// попадает в узкий просвет, зажатый между прядью/контуром и телом, и он остаётся
/// светлой каймой. Удаляется связная область пикселей, близких к фону (`tol`), если
/// одновременно: она тонкая (глубина от её края не больше `max_width`, т.е. ширина
/// до `2*max_width` px) и подходит к прозрачности ближе чем на `reach` px.
/// Толстые светлые области (белая обувь, белки глаз) и далёкие от границы блики
/// не затрагиваются. Возвращает число удалённых пикселей.
pub fn remove_thin_pockets(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    bg: [u8; 3],
    tol: u8,
    reach: u32,
    max_width: u32,
) -> usize {
    use std::collections::VecDeque;
    assert_eq!(rgba.len(), w * h * 4);
    if reach == 0 || max_width == 0 {
        return 0;
    }
    let n = w * h;
    let near: Vec<bool> = (0..n)
        .map(|p| rgba[p * 4 + 3] != 0 && dist(rgba, p * 4, bg) <= tol)
        .collect();
    let neighbors = |p: usize| {
        let (x, y) = (p % w, p / w);
        [
            if x > 0 { p - 1 } else { usize::MAX },
            if x + 1 < w { p + 1 } else { usize::MAX },
            if y > 0 { p - w } else { usize::MAX },
            if y + 1 < h { p + w } else { usize::MAX },
        ]
    };

    // Расстояние до прозрачности, считаем до `reach`.
    let mut d = vec![u32::MAX; n];
    let mut q: VecDeque<usize> = VecDeque::new();
    for p in 0..n {
        if rgba[p * 4 + 3] == 0 {
            d[p] = 0;
            q.push_back(p);
        }
    }
    if q.is_empty() {
        return 0;
    }
    while let Some(p) = q.pop_front() {
        if d[p] >= reach {
            continue;
        }
        for t in neighbors(p) {
            if t != usize::MAX && d[t] == u32::MAX {
                d[t] = d[p] + 1;
                q.push_back(t);
            }
        }
    }

    let mut seen = vec![false; n];
    let mut inner = vec![0u32; n]; // глубина пикселя внутри своей области
    let mut removed = 0;
    for start in 0..n {
        if !near[start] || seen[start] {
            continue;
        }
        // Связная область (4-связность).
        let mut cells = vec![start];
        seen[start] = true;
        let mut i = 0;
        while i < cells.len() {
            let p = cells[i];
            i += 1;
            for t in neighbors(p) {
                if t != usize::MAX && near[t] && !seen[t] {
                    seen[t] = true;
                    cells.push(t);
                }
            }
        }
        // Подходит ли область к границе достаточно близко?
        if !cells.iter().any(|&p| d[p] != u32::MAX && d[p] <= reach) {
            continue;
        }
        // Глубина внутри области: от пикселей, у которых есть сосед вне области.
        let mut bq: VecDeque<usize> = VecDeque::new();
        for &p in &cells {
            let edge = neighbors(p).iter().any(|&t| t != usize::MAX && !near[t]);
            if edge {
                inner[p] = 1;
                bq.push_back(p);
            }
        }
        let mut max_depth = 0;
        if bq.is_empty() {
            max_depth = u32::MAX; // область без границы (вся картинка): не трогаем
        }
        while let Some(p) = bq.pop_front() {
            max_depth = max_depth.max(inner[p]);
            if max_depth > max_width {
                break;
            }
            for t in neighbors(p) {
                if t != usize::MAX && near[t] && inner[t] == 0 {
                    inner[t] = inner[p] + 1;
                    bq.push_back(t);
                }
            }
        }
        // Если выход по break, глубина больше max_width: область толстая.
        let thin = max_depth <= max_width;
        for &p in &cells {
            inner[p] = 0;
        }
        if thin {
            for &p in &cells {
                rgba[p * 4..p * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            removed += cells.len();
        }
    }
    removed
}

/// Минимальный контраст (евклидово расстояние в RGB) между цветом объекта и фоном,
/// при котором доля фона в краевом пикселе ещё можно оценить. У почти белого
/// объекта на белом фоне (кроссовки) оценка ненадёжна, такие пиксели не трогаем.
const MIN_CONTRAST: f32 = 60.0;

/// Глубина полосы для `defringe` по умолчанию: чем больше изображение, тем шире
/// сглаженный край. Подобрана по метрике на JPG 1024x1536: 4 px; не меньше 2.
pub fn auto_defringe_depth(w: usize, h: usize) -> u32 {
    ((w.max(h) / 384) as u32).max(2)
}

/// Убирает светлую кайму вдоль границы вырезанного фона (матирование краевых
/// пикселей). Краевой пиксель — смесь цвета объекта `F` и фона: `P = a*F + (1-a)*bg`.
/// Для каждого пикселя в полосе шириной `depth` px от границы:
/// 1. `F` берётся как цвет ближайших пикселей глубже полосы (передаётся слой за слоем);
/// 2. доля объекта `a` оценивается проекцией `P - bg` на `F - bg`;
/// 3. `a >= 0.92` — пиксель считается чистым и не меняется; `a <= 0.08` — удаляется;
///    иначе цвет очищается от примеси фона `(P - (1-a)*bg) / a`, а прозрачность
///    становится `a`. Дальнейшее уменьшение учтёт её как покрытие ячейки.
///
/// Пиксели, у которых нет «глубокого» соседа (тонкие линии) или чей `F` слишком
/// близок к фону (`MIN_CONTRAST`), остаются как есть. Возвращает число изменённых
/// пикселей. Вызывать после `remove_background`.
pub fn defringe(rgba: &mut [u8], w: usize, h: usize, bg: [u8; 3], depth: u32) -> usize {
    use std::collections::VecDeque;
    assert_eq!(rgba.len(), w * h * 4);
    if depth == 0 {
        return 0;
    }
    let n = w * h;

    // 1. Расстояние (4-связность) от прозрачных пикселей; считаем до depth+1.
    let mut d = vec![u32::MAX; n];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for p in 0..n {
        if rgba[p * 4 + 3] == 0 {
            d[p] = 0;
            queue.push_back(p);
        }
    }
    if queue.is_empty() {
        return 0;
    }
    let neighbors = |p: usize| {
        let (x, y) = (p % w, p / w);
        [
            if x > 0 { p - 1 } else { usize::MAX },
            if x + 1 < w { p + 1 } else { usize::MAX },
            if y > 0 { p - w } else { usize::MAX },
            if y + 1 < h { p + w } else { usize::MAX },
        ]
    };
    while let Some(p) = queue.pop_front() {
        let nd = d[p] + 1;
        if nd > depth + 1 {
            continue;
        }
        for q in neighbors(p) {
            if q != usize::MAX && d[q] == u32::MAX {
                d[q] = nd;
                queue.push_back(q);
            }
        }
    }

    // Пиксели полосы по слоям: levels[l] — расстояние l (1..=depth).
    let mut levels: Vec<Vec<usize>> = vec![Vec::new(); depth as usize + 1];
    for p in 0..n {
        if d[p] >= 1 && d[p] <= depth {
            levels[d[p] as usize].push(p);
        }
    }

    // 2-3. Идём от глубоких слоёв к границе: оценка F, затем доля объекта.
    let mut fcol = vec![[0u8; 3]; n];
    let mut has_f = vec![false; n];
    let mut changed = 0;
    for lv in (1..=depth as usize).rev() {
        for &p in &levels[lv] {
            let mut sum = [0f32; 3];
            let mut cnt = 0f32;
            for q in neighbors(p) {
                if q == usize::MAX {
                    continue;
                }
                let deeper = d[q] as usize == lv + 1 || (lv == depth as usize && d[q] > depth);
                if !deeper {
                    continue;
                }
                let f = if d[q] > depth {
                    [rgba[q * 4], rgba[q * 4 + 1], rgba[q * 4 + 2]] // глубже полосы: свой цвет
                } else if has_f[q] {
                    fcol[q]
                } else {
                    continue;
                };
                for c in 0..3 {
                    sum[c] += f[c] as f32;
                }
                cnt += 1.0;
            }
            if cnt == 0.0 {
                continue; // нет глубокого соседа: оставляем пиксель как есть
            }
            let f = [sum[0] / cnt, sum[1] / cnt, sum[2] / cnt];
            fcol[p] = [f[0].round() as u8, f[1].round() as u8, f[2].round() as u8];
            has_f[p] = true;

            let v = [
                f[0] - bg[0] as f32,
                f[1] - bg[1] as f32,
                f[2] - bg[2] as f32,
            ];
            let vv = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
            if vv < MIN_CONTRAST * MIN_CONTRAST {
                continue; // объект почти цвета фона: доля не определяется
            }
            let i = p * 4;
            let pc = [rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32];
            let dot = (pc[0] - bg[0] as f32) * v[0]
                + (pc[1] - bg[1] as f32) * v[1]
                + (pc[2] - bg[2] as f32) * v[2];
            let a = (dot / vv).clamp(0.0, 1.0);
            if a >= 0.92 {
                continue; // чистый пиксель объекта
            }
            if a <= 0.08 {
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
            } else {
                for c in 0..3 {
                    let un = (pc[c] - (1.0 - a) * bg[c] as f32) / a;
                    rgba[i + c] = un.clamp(0.0, 255.0).round() as u8;
                }
                rgba[i + 3] = (a * 255.0).round() as u8;
            }
            changed += 1;
        }
    }
    changed
}

/// Делает полностью прозрачными пиксели со слабой альфой (`alpha < cut`) — остатки
/// мягкой маски вокруг вырезанного объекта. Возвращает число обнулённых пикселей.
pub fn cut_faint_alpha(rgba: &mut [u8], cut: u8) -> usize {
    let mut n = 0;
    for p in rgba.as_chunks_mut::<4>().0 {
        if p[3] != 0 && p[3] < cut {
            p.copy_from_slice(&[0, 0, 0, 0]);
            n += 1;
        }
    }
    n
}

/// Удаляет изолированные группы непрозрачных ячеек (8-связность) размером меньше
/// `min_cells`. Самая большая группа сохраняется всегда, даже если она меньше порога.
/// Возвращает число удалённых ячеек. Ожидает бинарную альфу (0 или 255 после порога).
pub fn despeckle(rgba: &mut [u8], w: usize, h: usize, min_cells: usize) -> usize {
    assert_eq!(rgba.len(), w * h * 4);
    if min_cells <= 1 {
        return 0;
    }
    let solid = |p: usize| rgba[p * 4 + 3] != 0;
    let mut label = vec![u32::MAX; w * h];
    let mut comps: Vec<Vec<usize>> = Vec::new();

    for start in 0..w * h {
        if !solid(start) || label[start] != u32::MAX {
            continue;
        }
        let id = comps.len() as u32;
        let mut cells = Vec::new();
        let mut stack = vec![start];
        label[start] = id;
        while let Some(p) = stack.pop() {
            cells.push(p);
            let (x, y) = ((p % w) as isize, (p / w) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (nx, ny) = (x + dx, y + dy);
                    if (dx == 0 && dy == 0)
                        || nx < 0
                        || ny < 0
                        || nx >= w as isize
                        || ny >= h as isize
                    {
                        continue;
                    }
                    let q = ny as usize * w + nx as usize;
                    if solid(q) && label[q] == u32::MAX {
                        label[q] = id;
                        stack.push(q);
                    }
                }
            }
        }
        comps.push(cells);
    }

    let Some(largest) = (0..comps.len()).max_by_key(|&i| comps[i].len()) else {
        return 0;
    };
    let mut removed = 0;
    for (i, cells) in comps.iter().enumerate() {
        if i != largest && cells.len() < min_cells {
            for &p in cells {
                rgba[p * 4..p * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
            removed += cells.len();
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: usize, h: usize, c: [u8; 4]) -> Vec<u8> {
        c.iter().cycle().take(w * h * 4).copied().collect()
    }
    fn set(img: &mut [u8], w: usize, x: usize, y: usize, c: [u8; 4]) {
        let i = (y * w + x) * 4;
        img[i..i + 4].copy_from_slice(&c);
    }
    fn alpha(img: &[u8], w: usize, x: usize, y: usize) -> u8 {
        img[(y * w + x) * 4 + 3]
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    /// Чёрное кольцо 10x10 с белой «дыркой» внутри на белом фоне.
    fn ring() -> (Vec<u8>, usize, usize) {
        let (w, h) = (20, 20);
        let mut img = canvas(w, h, WHITE);
        for y in 5..15 {
            for x in 5..15 {
                set(&mut img, w, x, y, BLACK);
            }
        }
        for y in 8..12 {
            for x in 8..12 {
                set(&mut img, w, x, y, WHITE); // замкнутая белая область внутри
            }
        }
        (img, w, h)
    }

    #[test]
    fn detects_white_border() {
        let (img, w, h) = ring();
        assert_eq!(detect_background(&img, w, h), [255, 255, 255]);
    }

    #[test]
    fn removes_outer_background_keeps_enclosed_white() {
        let (mut img, w, h) = ring();
        let removed = remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(removed, 400 - 100); // всё, кроме квадрата 10x10 (включая дырку)
        assert_eq!(alpha(&img, w, 0, 0), 0);
        assert_eq!(alpha(&img, w, 9, 9), 255); // белая дырка внутри осталась непрозрачной
        assert_eq!(alpha(&img, w, 5, 5), 255);
        assert_eq!(&img[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn tolerance_handles_jpeg_noise() {
        let (mut img, w, h) = ring();
        set(&mut img, w, 1, 1, [248, 252, 250, 255]); // «грязный» белый
        remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(alpha(&img, w, 1, 1), 0);
        // а при нулевом допуске грязный пиксель остаётся
        let (mut img2, w, h) = ring();
        set(&mut img2, w, 1, 1, [248, 252, 250, 255]);
        remove_background(&mut img2, w, h, [255, 255, 255], 0, 0, 0);
        assert_eq!(alpha(&img2, w, 1, 1), 255);
    }

    #[test]
    fn grow_peels_light_fringe_but_not_dark_outline() {
        let (w, h) = (12, 12);
        let mut img = canvas(w, h, WHITE);
        for y in 3..9 {
            for x in 3..9 {
                set(&mut img, w, x, y, BLACK);
            }
        }
        // светлая кайма (ореол) вокруг квадрата
        for y in 2..10 {
            for x in 2..10 {
                if !(3..9).contains(&x) || !(3..9).contains(&y) {
                    set(&mut img, w, x, y, [235, 235, 235, 255]);
                }
            }
        }
        let mut a = img.clone();
        remove_background(&mut a, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(alpha(&a, w, 2, 2), 255); // без grow кайма остаётся
        let mut b = img.clone();
        remove_background(&mut b, w, h, [255, 255, 255], 10, 1, 0);
        assert_eq!(alpha(&b, w, 2, 2), 0); // с grow снята (235 в пределах 3*tol от белого)
        assert_eq!(alpha(&b, w, 3, 3), 255); // чёрный контур цел

        // серый 200 дальше 3*tol от фона: это уже цвет объекта, он остаётся
        let mut c = img.clone();
        for y in 2..10 {
            for x in 2..10 {
                if !(3..9).contains(&x) || !(3..9).contains(&y) {
                    set(&mut c, w, x, y, [200, 200, 200, 255]);
                }
            }
        }
        remove_background(&mut c, w, h, [255, 255, 255], 10, 1, 0);
        assert_eq!(alpha(&c, w, 2, 2), 255);
    }

    #[test]
    fn removes_large_enclosed_pocket_but_not_small_highlight() {
        let (w, h) = (30, 30);
        let mut img = canvas(w, h, WHITE);
        for y in 3..27 {
            for x in 3..27 {
                set(&mut img, w, x, y, BLACK);
            }
        }
        for y in 8..16 {
            for x in 8..16 {
                set(&mut img, w, x, y, WHITE); // замкнутый просвет 8x8 = 64
            }
        }
        for y in 20..22 {
            for x in 20..22 {
                set(&mut img, w, x, y, WHITE); // блик 2x2 = 4
            }
        }
        let mut off = img.clone();
        remove_background(&mut off, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(alpha(&off, w, 10, 10), 255); // по умолчанию просвет остаётся
        let mut on = img.clone();
        let removed = remove_background(&mut on, w, h, [255, 255, 255], 10, 0, 20);
        assert_eq!(alpha(&on, w, 10, 10), 0); // крупный просвет удалён
        assert_eq!(alpha(&on, w, 20, 20), 255); // блик 4 px цел
        assert_eq!(removed, 900 - 576 + 64);
    }

    #[test]
    fn existing_transparency_counts_as_background() {
        let (w, h) = (6, 6);
        let mut img = canvas(w, h, [0, 0, 0, 0]);
        set(&mut img, w, 2, 2, BLACK);
        set(&mut img, w, 3, 2, [255, 255, 255, 255]); // белый пиксель рядом с объектом
        remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(alpha(&img, w, 2, 2), 255);
        assert_eq!(alpha(&img, w, 3, 2), 0); // белый, связан с прозрачным краем -> фон
    }

    #[test]
    fn cut_faint_alpha_only_touches_weak_pixels() {
        let mut img = vec![10, 20, 30, 5, 10, 20, 30, 16, 1, 2, 3, 255, 0, 0, 0, 0];
        let n = cut_faint_alpha(&mut img, 16);
        assert_eq!(n, 1);
        assert_eq!(&img[0..4], &[0, 0, 0, 0]);
        assert_eq!(&img[4..8], &[10, 20, 30, 16]);
        assert_eq!(&img[8..12], &[1, 2, 3, 255]);
    }

    fn grid(w: usize, h: usize, cells: &[(usize, usize)]) -> Vec<u8> {
        let mut img = canvas(w, h, [0, 0, 0, 0]);
        for &(x, y) in cells {
            set(&mut img, w, x, y, [10, 20, 30, 255]);
        }
        img
    }

    #[test]
    fn despeckle_removes_small_islands_keeps_main_body() {
        let mut cells = vec![];
        for y in 2..7 {
            for x in 2..7 {
                cells.push((x, y)); // тело 25 ячеек
            }
        }
        cells.push((12, 12)); // одиночная пылинка
        cells.push((14, 2));
        cells.push((15, 3)); // две ячейки по диагонали = одна 8-связная группа из 2
        let mut img = grid(18, 18, &cells);
        let removed = despeckle(&mut img, 18, 18, 3);
        assert_eq!(removed, 3);
        assert_eq!(alpha(&img, 18, 12, 12), 0);
        assert_eq!(alpha(&img, 18, 14, 2), 0);
        assert_eq!(alpha(&img, 18, 4, 4), 255);
    }

    #[test]
    fn despeckle_keeps_diagonal_connection_and_large_enough_groups() {
        let mut cells = vec![];
        for y in 2..7 {
            for x in 2..7 {
                cells.push((x, y));
            }
        }
        cells.push((7, 7)); // касается тела по диагонали -> часть тела
        cells.push((12, 12));
        cells.push((13, 12)); // группа из 2
        let mut img = grid(18, 18, &cells);
        let removed = despeckle(&mut img, 18, 18, 2); // порог 2: группы из 2 остаются
        assert_eq!(removed, 0); // одиночных нет: (7,7) присоединён к телу
        assert_eq!(alpha(&img, 18, 7, 7), 255);
        assert_eq!(alpha(&img, 18, 12, 12), 255);
    }

    #[test]
    fn despeckle_never_deletes_the_largest_group_and_can_be_disabled() {
        let mut img = grid(8, 8, &[(1, 1), (5, 5)]); // две одиночные, обе меньше порога
        let removed = despeckle(&mut img, 8, 8, 10);
        assert_eq!(removed, 1); // одна из них (наибольшая из равных) остаётся
        let survivors = (0..64).filter(|&p| img[p * 4 + 3] != 0).count();
        assert_eq!(survivors, 1);
        let mut img2 = grid(8, 8, &[(1, 1)]);
        assert_eq!(despeckle(&mut img2, 8, 8, 0), 0);
        assert_eq!(despeckle(&mut img2, 8, 8, 1), 0);
        let mut empty = grid(4, 4, &[]);
        assert_eq!(despeckle(&mut empty, 4, 4, 5), 0);
    }

    /// Чёрный квадрат 8x8 на белом фоне с сглаженной границей шириной 1 px:
    /// пиксель кольца = a*чёрный + (1-a)*белый.
    fn aa_square(a: f32, inner: [u8; 4]) -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut img = canvas(w, h, WHITE);
        let edge = [
            (inner[0] as f32 * a + 255.0 * (1.0 - a)).round() as u8,
            (inner[1] as f32 * a + 255.0 * (1.0 - a)).round() as u8,
            (inner[2] as f32 * a + 255.0 * (1.0 - a)).round() as u8,
            255,
        ];
        for y in 3..13 {
            for x in 3..13 {
                let on_ring = x == 3 || x == 12 || y == 3 || y == 12;
                set(&mut img, w, x, y, if on_ring { edge } else { inner });
            }
        }
        (img, w, h)
    }

    #[test]
    fn defringe_restores_alpha_and_color_of_gray_edge() {
        let (mut img, w, h) = aa_square(0.5, [0, 0, 0, 255]); // кольцо серое 128
        remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(alpha(&img, w, 3, 8), 255); // серое кольцо пережило удаление фона
        let n = defringe(&mut img, w, h, [255, 255, 255], 2);
        assert!(n > 0);
        let i = (8 * w + 3) * 4;
        assert!(
            (img[i + 3] as i32 - 128).abs() <= 3,
            "альфа ~0.5, получено {}",
            img[i + 3]
        );
        assert!(
            img[i] < 8 && img[i + 1] < 8 && img[i + 2] < 8,
            "цвет очищен до чёрного: {:?}",
            &img[i..i + 4]
        );
        assert_eq!(&img[(8 * w + 8) * 4..(8 * w + 8) * 4 + 4], &[0, 0, 0, 255]); // середина цела
    }

    #[test]
    fn defringe_drops_very_light_halo_and_keeps_clean_edge() {
        let (mut halo, w, h) = aa_square(0.05, [0, 0, 0, 255]); // почти белый ореол (243)
        remove_background(&mut halo, w, h, [255, 255, 255], 5, 0, 0);
        defringe(&mut halo, w, h, [255, 255, 255], 2);
        assert_eq!(alpha(&halo, w, 3, 8), 0); // ореол убран полностью

        let (mut clean_edge, w, h) = aa_square(0.97, [0, 0, 0, 255]);
        remove_background(&mut clean_edge, w, h, [255, 255, 255], 5, 0, 0);
        let before = clean_edge.clone();
        defringe(&mut clean_edge, w, h, [255, 255, 255], 2);
        assert_eq!(before, clean_edge); // почти чистый край не меняется
    }

    #[test]
    fn defringe_recovers_colored_object_edge() {
        let yellow = [240, 200, 30, 255];
        let (mut img, w, h) = aa_square(0.5, yellow);
        remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        defringe(&mut img, w, h, [255, 255, 255], 2);
        let i = (8 * w + 3) * 4;
        assert!((img[i + 3] as i32 - 128).abs() <= 3);
        for c in 0..3 {
            assert!(
                (img[i + c] as i32 - yellow[c] as i32).abs() <= 4,
                "канал {c}: {:?}",
                &img[i..i + 4]
            );
        }
    }

    #[test]
    fn defringe_leaves_near_white_objects_and_thin_lines_alone() {
        // почти белый объект (контраст с фоном < MIN_CONTRAST)
        let (mut a, w, h) = aa_square(1.0, [235, 235, 235, 255]);
        remove_background(&mut a, w, h, [255, 255, 255], 5, 0, 0);
        let before = a.clone();
        assert_eq!(defringe(&mut a, w, h, [255, 255, 255], 2), 0);
        assert_eq!(before, a);
        // тонкая линия шириной 1 px: нет глубокого соседа
        let (w, h) = (12, 12);
        let mut b = canvas(w, h, [0, 0, 0, 0]);
        for x in 2..10 {
            set(&mut b, w, x, 6, [100, 100, 100, 255]);
        }
        let before = b.clone();
        assert_eq!(defringe(&mut b, w, h, [255, 255, 255], 2), 0);
        assert_eq!(before, b);
        // depth = 0 и отсутствие прозрачных пикселей
        let mut c = canvas(4, 4, BLACK);
        assert_eq!(defringe(&mut c, 4, 4, [255, 255, 255], 0), 0);
        assert_eq!(defringe(&mut c, 4, 4, [255, 255, 255], 3), 0);
    }

    #[test]
    fn auto_depth_scales_with_size() {
        assert_eq!(auto_defringe_depth(300, 400), 2);
        assert_eq!(auto_defringe_depth(1024, 1536), 4);
        assert_eq!(auto_defringe_depth(4000, 3000), 10);
    }

    #[test]
    fn thin_pockets_removed_near_edge_only() {
        let (w, h) = (40, 24);
        let mut img = canvas(w, h, WHITE);
        // чёрное тело
        for y in 2..22 {
            for x in 10..38 {
                set(&mut img, w, x, y, BLACK);
            }
        }
        // серая прядь x=5..6 и щель цвета фона x=7..9 (ширина 3) между прядью и телом
        for y in 4..20 {
            set(&mut img, w, 5, y, [150, 150, 150, 255]);
            set(&mut img, w, 6, y, [150, 150, 150, 255]);
        }
        // замыкаем щель сверху и снизу, чтобы заливка от краёв в неё не попала
        for x in 5..10 {
            set(&mut img, w, x, 3, [150, 150, 150, 255]);
            set(&mut img, w, x, 20, [150, 150, 150, 255]);
        }
        // толстый блик 6x6 далеко от края и тонкий блик 1x6 далеко от края
        for y in 8..14 {
            for x in 22..28 {
                set(&mut img, w, x, y, WHITE);
            }
        }
        for y in 8..14 {
            set(&mut img, w, 32, y, WHITE); // тонкий блик 1x6, от границы дальше reach
        }
        remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        assert_eq!(alpha(&img, w, 8, 10), 255); // щель пережила заливку
        let n = remove_thin_pockets(&mut img, w, h, [255, 255, 255], 10, 3, 2);
        assert_eq!(n, 3 * 16); // вся щель 3x16 удалена
        assert_eq!(alpha(&img, w, 8, 10), 0);
        assert_eq!(alpha(&img, w, 24, 10), 255); // толстый блик цел
        assert_eq!(alpha(&img, w, 32, 10), 255); // тонкий блик далеко от границы цел
        assert_eq!(alpha(&img, w, 5, 10), 255); // сама прядь не тронута
    }

    #[test]
    fn thin_pockets_keep_thick_regions_and_respect_disabled_params() {
        let (w, h) = (30, 30);
        let mut img = canvas(w, h, WHITE);
        for y in 3..27 {
            for x in 3..27 {
                set(&mut img, w, x, y, BLACK);
            }
        }
        for y in 4..12 {
            for x in 4..12 {
                set(&mut img, w, x, y, WHITE); // толстый белый блок у самой границы: 8x8
            }
        }
        remove_background(&mut img, w, h, [255, 255, 255], 10, 0, 0);
        let before = img.clone();
        assert_eq!(
            remove_thin_pockets(&mut img, w, h, [255, 255, 255], 10, 3, 2),
            0
        );
        assert_eq!(before, img);
        assert_eq!(
            remove_thin_pockets(&mut img, w, h, [255, 255, 255], 10, 0, 2),
            0
        );
        assert_eq!(
            remove_thin_pockets(&mut img, w, h, [255, 255, 255], 10, 3, 0),
            0
        );
    }
}
