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
/// `px` — первые три байта RGBA-пикселя (альфа не учитывается).
#[inline]
fn dist_px(px: &[u8], bg: [u8; 3]) -> u8 {
    let d = |a: u8, b: u8| a.abs_diff(b);
    d(px[0], bg[0]).max(d(px[1], bg[1])).max(d(px[2], bg[2]))
}

/// То же, но по индексу начала пикселя в сыром буфере.
#[inline]
fn dist(rgba: &[u8], i: usize, bg: [u8; 3]) -> u8 {
    dist_px(&rgba[i..], bg)
}

/// Пиксель считается фоном, если он прозрачный или цвет в пределах `tol` от `bg`.
#[inline]
fn is_bg(rgba: &[u8], i: usize, bg: [u8; 3], tol: u8) -> bool {
    rgba[i + 3] == 0 || dist(rgba, i, bg) <= tol
}

/// Плотная маска пикселей: один бит на пиксель.
///
/// Заменяет `Vec<bool>` (байт на пиксель): на 24 Мп это 0.75 МБ вместо 6 МБ на
/// каждую маску, а `reset` затирает только реально используемые слова. Буфер
/// переиспользуется [`Scratch`] между вызовами.
#[derive(Default)]
pub struct Bitset {
    words: Vec<u64>,
    len: usize,
}

impl Bitset {
    /// Готовит маску на `len` пикселей, все биты сброшены.
    fn reset(&mut self, len: usize) {
        let words = len.div_ceil(64);
        if self.words.len() < words {
            self.words.resize(words, 0);
        }
        self.words[..words].fill(0);
        self.len = len;
    }

    #[inline]
    fn get(&self, i: usize) -> bool {
        debug_assert!(i < self.len, "битсет: {i} при длине {}", self.len);
        (self.words[i >> 6] >> (i & 63)) & 1 != 0
    }

    #[inline]
    fn set(&mut self, i: usize) {
        debug_assert!(i < self.len, "битсет: {i} при длине {}", self.len);
        self.words[i >> 6] |= 1 << (i & 63);
    }
}

/// Переиспользуемые буферы очистки.
///
/// Один экземпляр на процесс (GUI-воркер держит его между генерациями): на 24 Мп
/// это десятки мегабайт, которые иначе выделялись бы заново на каждый клик.
/// Все проходы обязаны затирать буферы при входе — состояние между вызовами не
/// переносится (тест `scratch_reuse_is_transparent`).
#[derive(Default)]
pub struct Scratch {
    /// Маска «фон убран» (заливка + просветы + кайма).
    gone: Bitset,
    /// Маска «пиксель уже просмотрен» в обходе просветов.
    seen: Bitset,
    /// Маска «цвет близок к фону» для тонких щелей.
    near: Bitset,
    /// Маска «у пикселя есть цвет глубокого слоя» для матирования.
    has_f: Bitset,
    /// Расстояние до прозрачности в пикселях (`u16::MAX` — недостижимо).
    dist: Vec<u16>,
    /// Глубина внутри области (тонкие щели) или номер слоя (матирование).
    depth: Vec<u16>,
    /// Стек/очередь обхода.
    todo: Vec<u32>,
    /// Список пикселей одной связной области или одного слоя.
    cells: Vec<u32>,
    /// Смещения слоёв в [`Scratch::levels`]: слой `l` — `levels[start[l]..start[l + 1]]`.
    level_start: Vec<u32>,
    /// Курсор заполнения [`Scratch::levels`] (по слоям).
    level_cursor: Vec<u32>,
    /// Пиксели по слоям полосы матирования, плоским списком.
    levels: Vec<u32>,
    /// Цвет «глубокого» слоя для матирования (3 байта на пиксель).
    fcol: Vec<[u8; 3]>,
}

/// Расстояние (4-связность) от каждого пикселя до ближайшего прозрачного.
///
/// BFS от прозрачных пикселей с ограничением `cap`: заливка не уходит дальше
/// `cap + 1` шагов, поэтому на 24 Мп обходится только полоса у границы объекта,
/// а не всё изображение. `todo` — переиспользуемая очередь (`u32` вместо
/// `usize`), `dist` заполняется `u16::MAX` для недостижимых.
fn distance_to_transparent(
    rgba: &[u8],
    w: usize,
    h: usize,
    cap: u16,
    dist: &mut Vec<u16>,
    todo: &mut Vec<u32>,
) {
    dist.clear();
    dist.resize(w * h, u16::MAX);
    todo.clear();
    for p in 0..w * h {
        if rgba[p * 4 + 3] == 0 {
            dist[p] = 0;
            todo.push(p as u32);
        }
    }
    let mut head = 0;
    while head < todo.len() {
        let p = todo[head] as usize;
        head += 1;
        let next = dist[p] + 1;
        if next > cap {
            continue;
        }
        let (x, y) = (p % w, p / w);
        let visit = |q: usize, dist: &mut Vec<u16>, todo: &mut Vec<u32>| {
            if dist[q] == u16::MAX {
                dist[q] = next;
                todo.push(q as u32);
            }
        };
        if x > 0 {
            visit(p - 1, dist, todo);
        }
        if x + 1 < w {
            visit(p + 1, dist, todo);
        }
        if y > 0 {
            visit(p - w, dist, todo);
        }
        if y + 1 < h {
            visit(p + w, dist, todo);
        }
    }
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
    let mut scratch = Scratch::default();
    remove_background_in(rgba, w, h, bg, tol, grow, min_hole, &mut scratch)
}

/// То же, что [`remove_background`], с переиспользуемыми буферами.
///
/// Заливка scanline: стек хранит только затравки строк (порядка `O(w)`), а не
/// каждый пиксель, поэтому на 24 Мп стек не растёт до размера изображения.
pub fn remove_background_in(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    bg: [u8; 3],
    tol: u8,
    grow: u32,
    min_hole: usize,
    scratch: &mut Scratch,
) -> usize {
    assert_eq!(rgba.len(), w * h * 4);
    let n = w * h;
    let Scratch {
        gone,
        seen,
        todo,
        cells,
        ..
    } = scratch;
    gone.reset(n);
    todo.clear();

    // Затравки: все граничные пиксели, подходящие под фон. Помечаем их не здесь,
    // а при заливке прогона — иначе повторно затравленный пиксель стоил бы
    // повторного прохода по строке.
    let push_seed = |p: usize, gone: &Bitset, todo: &mut Vec<u32>| {
        if !gone.get(p) && is_bg(rgba, p * 4, bg, tol) {
            todo.push(p as u32);
        }
    };
    for x in 0..w {
        push_seed(x, gone, todo);
        push_seed((h - 1) * w + x, gone, todo);
    }
    for y in 0..h {
        push_seed(y * w, gone, todo);
        push_seed(y * w + w - 1, gone, todo);
    }

    // Заливка прогонами: каждый вход в цикл либо красит хотя бы один новый
    // пиксель, либо сразу выходит, поэтому всего работы `O(n)`.
    while let Some(start) = todo.pop() {
        let p = start as usize;
        if gone.get(p) {
            continue; // прогон уже закрашен другой затравкой
        }
        if !is_bg(rgba, p * 4, bg, tol) {
            continue; // страховка от затравки, переставшей быть фоном
        }
        let y = p / w;
        let mut xl = p % w;
        let mut xr = xl;
        while xl > 0 {
            let q = y * w + xl - 1;
            if gone.get(q) || !is_bg(rgba, q * 4, bg, tol) {
                break;
            }
            xl -= 1;
        }
        while xr + 1 < w {
            let q = y * w + xr + 1;
            if gone.get(q) || !is_bg(rgba, q * 4, bg, tol) {
                break;
            }
            xr += 1;
        }
        for x in xl..=xr {
            gone.set(y * w + x);
        }
        // Соседние строки: затравляем по одному пикселю на прогон, а не на
        // каждый пиксель — иначе стек раздувается до размера изображения.
        for row in [y.checked_sub(1), Some(y + 1)].into_iter().flatten() {
            if row >= h {
                continue;
            }
            let mut x = xl;
            while x <= xr {
                let q = row * w + x;
                if gone.get(q) || !is_bg(rgba, q * 4, bg, tol) {
                    x += 1;
                    continue;
                }
                todo.push(q as u32);
                // Пропускаем весь ещё не закрашенный прогон этого ряда.
                while x <= xr {
                    let q = row * w + x;
                    if gone.get(q) || !is_bg(rgba, q * 4, bg, tol) {
                        break;
                    }
                    x += 1;
                }
            }
        }
    }

    // Замкнутые просветы фона: крупные связные области цвета фона внутри объекта.
    if min_hole > 0 {
        seen.reset(n);
        for start in 0..n {
            if gone.get(start) || seen.get(start) || !is_bg(rgba, start * 4, bg, tol) {
                continue;
            }
            cells.clear();
            cells.push(start as u32);
            seen.set(start);
            let mut head = 0;
            while head < cells.len() {
                let p = cells[head] as usize;
                head += 1;
                let (x, y) = (p % w, p / w);
                let visit = |q: usize, seen: &mut Bitset, cells: &mut Vec<u32>| {
                    if !gone.get(q) && !seen.get(q) && is_bg(rgba, q * 4, bg, tol) {
                        seen.set(q);
                        cells.push(q as u32);
                    }
                };
                if x > 0 {
                    visit(p - 1, seen, cells);
                }
                if x + 1 < w {
                    visit(p + 1, seen, cells);
                }
                if y > 0 {
                    visit(p - w, seen, cells);
                }
                if y + 1 < h {
                    visit(p + w, seen, cells);
                }
            }
            if cells.len() >= min_hole {
                for &q in cells.iter() {
                    gone.set(q as usize);
                }
            }
        }
    }

    // Снятие светлой каймы слоями вдоль найденной границы.
    let soft = tol.saturating_mul(3);
    for _ in 0..grow {
        cells.clear();
        for y in 0..h {
            for x in 0..w {
                let p = y * w + x;
                if gone.get(p) || dist(rgba, p * 4, bg) > soft {
                    continue;
                }
                let touches = (x > 0 && gone.get(p - 1))
                    || (x + 1 < w && gone.get(p + 1))
                    || (y > 0 && gone.get(p - w))
                    || (y + 1 < h && gone.get(p + w));
                if touches {
                    cells.push(p as u32);
                }
            }
        }
        if cells.is_empty() {
            break;
        }
        for &q in cells.iter() {
            gone.set(q as usize);
        }
    }

    let mut removed = 0;
    for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
        if gone.get(i) {
            if px[3] != 0 {
                removed += 1;
            }
            px.copy_from_slice(&[0, 0, 0, 0]);
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
    let mut scratch = Scratch::default();
    remove_thin_pockets_in(rgba, w, h, bg, tol, reach, max_width, &mut scratch)
}

/// То же, что [`remove_thin_pockets`], с переиспользуемыми буферами.
///
/// Расстояния и глубины хранятся в `u16` (по построению не больше `reach` и
/// `max_width`, оба малы), маски — битсетом, списки пикселей — `Vec<u32>`.
pub fn remove_thin_pockets_in(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    bg: [u8; 3],
    tol: u8,
    reach: u32,
    max_width: u32,
    scratch: &mut Scratch,
) -> usize {
    assert_eq!(rgba.len(), w * h * 4);
    if reach == 0 || max_width == 0 {
        return 0;
    }
    debug_assert!(reach <= u16::MAX as u32, "reach {reach} не влезает в u16");
    debug_assert!(
        max_width <= u16::MAX as u32,
        "max_width {max_width} не влезает в u16"
    );
    let n = w * h;
    let reach16 = reach as u16;
    let width16 = max_width as u16;
    let Scratch {
        near,
        seen,
        dist,
        depth,
        todo,
        cells,
        ..
    } = scratch;

    near.reset(n);
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        if px[3] != 0 && dist_px(px, bg) <= tol {
            near.set(i);
        }
    }

    // Расстояние до прозрачности, считаем до `reach`.
    distance_to_transparent(rgba, w, h, reach16, dist, todo);
    if !dist.iter().any(|&d| d == 0) {
        return 0;
    }

    seen.reset(n);
    depth.clear();
    depth.resize(n, 0);
    let mut removed = 0;

    for start in 0..n {
        if !near.get(start) || seen.get(start) {
            continue;
        }
        // Связная область (4-связность).
        cells.clear();
        cells.push(start as u32);
        seen.set(start);
        let mut head = 0;
        while head < cells.len() {
            let p = cells[head] as usize;
            head += 1;
            let (x, y) = (p % w, p / w);
            let visit = |q: usize, seen: &mut Bitset, cells: &mut Vec<u32>| {
                if near.get(q) && !seen.get(q) {
                    seen.set(q);
                    cells.push(q as u32);
                }
            };
            if x > 0 {
                visit(p - 1, seen, cells);
            }
            if x + 1 < w {
                visit(p + 1, seen, cells);
            }
            if y > 0 {
                visit(p - w, seen, cells);
            }
            if y + 1 < h {
                visit(p + w, seen, cells);
            }
        }

        // Подходит ли область к границе достаточно близко?
        let close = cells
            .iter()
            .any(|&p| dist[p as usize] != u16::MAX && dist[p as usize] <= reach16);
        if !close {
            continue;
        }

        // Глубина внутри области: от пикселей, у которых есть сосед вне области.
        todo.clear();
        for &p in cells.iter() {
            let p = p as usize;
            let (x, y) = (p % w, p / w);
            let edge = (x > 0 && !near.get(p - 1))
                || (x + 1 < w && !near.get(p + 1))
                || (y > 0 && !near.get(p - w))
                || (y + 1 < h && !near.get(p + w));
            if edge {
                depth[p] = 1;
                todo.push(p as u32);
            }
        }
        let mut max_depth = 0u16;
        if todo.is_empty() {
            max_depth = u16::MAX; // область без границы (вся картинка): не трогаем
        }
        let mut head = 0;
        while head < todo.len() {
            let p = todo[head] as usize;
            head += 1;
            max_depth = max_depth.max(depth[p]);
            if max_depth > width16 {
                break;
            }
            let (x, y) = (p % w, p / w);
            let next = depth[p] + 1;
            let visit = |q: usize, depth: &mut Vec<u16>, todo: &mut Vec<u32>| {
                if near.get(q) && depth[q] == 0 {
                    depth[q] = next;
                    todo.push(q as u32);
                }
            };
            if x > 0 {
                visit(p - 1, depth, todo);
            }
            if x + 1 < w {
                visit(p + 1, depth, todo);
            }
            if y > 0 {
                visit(p - w, depth, todo);
            }
            if y + 1 < h {
                visit(p + w, depth, todo);
            }
        }
        // Если выход по break, глубина больше max_width: область толстая.
        let thin = max_depth <= width16;
        for &p in cells.iter() {
            depth[p as usize] = 0;
        }
        if thin {
            for &p in cells.iter() {
                let i = p as usize * 4;
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
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
    let mut scratch = Scratch::default();
    defringe_in(rgba, w, h, bg, depth, &mut scratch)
}

/// То же, что [`defringe`], с переиспользуемыми буферами.
///
/// Расстояния — `u16` (по построению не больше `depth + 1`), слои — плоский
/// список `u32` со смещениями, цвет глубокого слоя — 3 байта на пиксель,
/// признак «цвет есть» — битсет. Флаг `has_f` сбрасывается на входе, поэтому
/// значения `fcol` от прошлого вызова наружу не просачиваются.
pub fn defringe_in(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    bg: [u8; 3],
    depth: u32,
    scratch: &mut Scratch,
) -> usize {
    assert_eq!(rgba.len(), w * h * 4);
    if depth == 0 {
        return 0;
    }
    debug_assert!(depth <= u16::MAX as u32 - 2, "depth {depth} не влезает в u16");
    let n = w * h;
    let depth_us = depth as usize;
    let depth16 = depth as u16;
    let Scratch {
        dist,
        has_f,
        todo,
        level_start,
        level_cursor,
        levels,
        fcol,
        ..
    } = scratch;

    // 1. Расстояние (4-связность) от прозрачных пикселей; считаем до depth+1.
    distance_to_transparent(rgba, w, h, depth16 + 1, dist, todo);
    if !dist.iter().any(|&d| d == 0) {
        return 0;
    }

    // Пиксели полосы по слоям: `levels[start[l]..start[l + 1]]` — слой `l`.
    level_start.clear();
    level_start.resize(depth_us + 2, 0);
    for &dd in dist.iter().take(n) {
        if dd >= 1 && dd as usize <= depth_us {
            level_start[dd as usize + 1] += 1;
        }
    }
    for l in 1..=depth_us + 1 {
        level_start[l] += level_start[l - 1];
    }
    levels.clear();
    levels.resize(level_start[depth_us + 1] as usize, 0);
    level_cursor.clear();
    level_cursor.extend_from_slice(&level_start[..=depth_us]);
    for p in 0..n {
        let dd = dist[p];
        if dd >= 1 && dd as usize <= depth_us {
            let l = dd as usize;
            let idx = level_cursor[l] as usize;
            levels[idx] = p as u32;
            level_cursor[l] += 1;
        }
    }

    has_f.reset(n);
    if fcol.len() < n {
        fcol.resize(n, [0u8; 3]);
    }
    let mut changed = 0;

    // 2-3. Идём от глубоких слоёв к границе: оценка F, затем доля объекта.
    for lv in (1..=depth_us).rev() {
        let from = level_start[lv] as usize;
        let to = level_start[lv + 1] as usize;
        for &p in &levels[from..to] {
            let p = p as usize;
            let (x, y) = (p % w, p / w);
            let mut sum = [0f32; 3];
            let mut cnt = 0f32;
            let gather = |q: usize, sum: &mut [f32; 3], cnt: &mut f32| {
                let dq = dist[q] as usize;
                let deeper = dq == lv + 1 || (lv == depth_us && dq > depth_us);
                if !deeper {
                    return;
                }
                let f = if dq > depth_us {
                    [rgba[q * 4], rgba[q * 4 + 1], rgba[q * 4 + 2]] // глубже полосы: свой цвет
                } else if has_f.get(q) {
                    fcol[q]
                } else {
                    return;
                };
                for c in 0..3 {
                    sum[c] += f[c] as f32;
                }
                *cnt += 1.0;
            };
            if x > 0 {
                gather(p - 1, &mut sum, &mut cnt);
            }
            if x + 1 < w {
                gather(p + 1, &mut sum, &mut cnt);
            }
            if y > 0 {
                gather(p - w, &mut sum, &mut cnt);
            }
            if y + 1 < h {
                gather(p + w, &mut sum, &mut cnt);
            }
            if cnt == 0.0 {
                continue; // нет глубокого соседа: оставляем пиксель как есть
            }
            let f = [sum[0] / cnt, sum[1] / cnt, sum[2] / cnt];
            fcol[p] = [f[0].round() as u8, f[1].round() as u8, f[2].round() as u8];
            has_f.set(p);

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

    /// Битсет обязан совпадать с `Vec<bool>` на любых длинах, включая границы
    /// слов, и повторный `reset` — полностью затирать.
    #[test]
    fn bitset_matches_bools() {
        let mut bits = Bitset::default();
        for len in [0usize, 1, 63, 64, 65, 127, 128, 129, 1000] {
            bits.reset(len);
            let mut want = vec![false; len];
            let mut state = 0x1234_5678u32 ^ len as u32;
            for i in 0..len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                if state & 1 != 0 {
                    bits.set(i);
                    want[i] = true;
                }
            }
            for i in 0..len {
                assert_eq!(bits.get(i), want[i], "длина {len}, бит {i}");
            }
            bits.reset(len);
            for i in 0..len {
                assert!(!bits.get(i), "reset не затёр бит {i} при длине {len}");
            }
        }
    }

    /// Эталонная заливка (BFS от границ, как было до scanline-версии):
    /// множество удалённых пикселей обязано совпасть.
    #[test]
    fn flood_fill_matches_reference_bfs() {
        use std::collections::VecDeque;

        fn reference(rgba: &mut [u8], w: usize, h: usize, bg: [u8; 3], tol: u8) -> usize {
            let is_bg = |i: usize, rgba: &[u8]| {
                rgba[i * 4 + 3] == 0 || dist_px(&rgba[i * 4..i * 4 + 3], bg) <= tol
            };
            let n = w * h;
            let mut gone = vec![false; n];
            let mut queue = VecDeque::new();
            let seed = |p: usize, gone: &mut Vec<bool>, queue: &mut VecDeque<usize>| {
                if !gone[p] && is_bg(p, rgba) {
                    gone[p] = true;
                    queue.push_back(p);
                }
            };
            for x in 0..w {
                seed(x, &mut gone, &mut queue);
                seed((h - 1) * w + x, &mut gone, &mut queue);
            }
            for y in 0..h {
                seed(y * w, &mut gone, &mut queue);
                seed(y * w + w - 1, &mut gone, &mut queue);
            }
            while let Some(p) = queue.pop_front() {
                let (x, y) = (p % w, p / w);
                let visit = |q: usize, gone: &mut Vec<bool>, queue: &mut VecDeque<usize>| {
                    if !gone[q] && is_bg(q, rgba) {
                        gone[q] = true;
                        queue.push_back(q);
                    }
                };
                if x > 0 {
                    visit(p - 1, &mut gone, &mut queue);
                }
                if x + 1 < w {
                    visit(p + 1, &mut gone, &mut queue);
                }
                if y > 0 {
                    visit(p - w, &mut gone, &mut queue);
                }
                if y + 1 < h {
                    visit(p + w, &mut gone, &mut queue);
                }
            }
            let mut removed = 0;
            for (p, px) in rgba.chunks_exact_mut(4).enumerate() {
                if gone[p] {
                    if px[3] != 0 {
                        removed += 1;
                    }
                    px.copy_from_slice(&[0, 0, 0, 0]);
                }
            }
            removed
        }

        let (w, h) = (32, 32);
        let bg = [240, 236, 224];
        let mut scratch = Scratch::default();
        let mut state = 7u32;
        for case in 0..200 {
            let mut img = Vec::with_capacity(w * h * 4);
            for _ in 0..w * h {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let bgish = state % 5 != 0;
                let (r, g, b) = if bgish {
                    (
                        238 + (state >> 3) % 5,
                        234 + (state >> 7) % 5,
                        222 + (state >> 11) % 5,
                    )
                } else {
                    (
                        30 + (state >> 3) % 60,
                        60 + (state >> 7) % 40,
                        90 + (state >> 11) % 40,
                    )
                };
                let a = if state % 23 == 0 { 0 } else { 255 };
                img.extend_from_slice(&[r as u8, g as u8, b as u8, a]);
            }

            let mut want = img.clone();
            let want_count = reference(&mut want, w, h, bg, 10);
            let got_count = remove_background_in(&mut img, w, h, bg, 10, 0, 0, &mut scratch);
            assert_eq!(img, want, "случай {case}: разошлись пиксели заливки");
            assert_eq!(got_count, want_count, "случай {case}: разошёлся счётчик");
        }
    }

    /// Переиспользование `Scratch` не должно переносить состояние между
    /// вызовами, а `_in`-варианты обязаны давать то же, что одноразовые обёртки.
    #[test]
    fn scratch_reuse_is_transparent() {
        let (w, h) = (40, 30);
        let bg = [250, 250, 250];
        let mut src = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let inside = x > 8 && x < 32 && y > 5 && y < 25;
                let soft = x > 6 && x <= 8 && y > 4 && y < 26;
                let (r, g, b, a) = if inside {
                    (40, 70, 120, 255)
                } else if soft {
                    (200, 180, 160, 150)
                } else {
                    (250, 250, 250, 255)
                };
                src.extend_from_slice(&[r, g, b, a]);
            }
        }

        let mut scratch = Scratch::default();
        let mut runs = Vec::new();
        for _ in 0..3 {
            let mut img = src.clone();
            let a = remove_background_in(&mut img, w, h, bg, 12, 1, 20, &mut scratch);
            let b = remove_thin_pockets_in(&mut img, w, h, bg, 12, 4, 2, &mut scratch);
            let c = defringe_in(&mut img, w, h, bg, 3, &mut scratch);
            runs.push((img, a, b, c));
        }
        assert_eq!(runs[0].0, runs[1].0, "второй прогон с тем же Scratch");
        assert_eq!(runs[0].0, runs[2].0, "третий прогон с тем же Scratch");
        assert_eq!(
            (runs[0].1, runs[0].2, runs[0].3),
            (runs[1].1, runs[1].2, runs[1].3)
        );

        // Одноразовые обёртки обязаны дать тот же результат.
        let mut img = src.clone();
        let a = remove_background(&mut img, w, h, bg, 12, 1, 20);
        let b = remove_thin_pockets(&mut img, w, h, bg, 12, 4, 2);
        let c = defringe(&mut img, w, h, bg, 3);
        assert_eq!(img, runs[0].0, "обёртка и `_in` расходятся");
        assert_eq!((a, b, c), (runs[0].1, runs[0].2, runs[0].3));
    }

    /// Крайние значения `reach`/`depth` (0, 1, 254, 255) не должны переполнять
    /// `u16` и не должны ронять последовательность.
    #[test]
    fn distances_u16_edges() {
        let (w, h) = (1, 300);
        let mut img = vec![0u8; w * h * 4];
        for y in 0..h {
            let c: [u8; 4] = if y % 3 == 0 {
                [0, 0, 0, 0]
            } else {
                [250, 250, 250, 255]
            };
            img[y * 4..y * 4 + 4].copy_from_slice(&c);
        }

        let mut scratch = Scratch::default();
        for reach in [0u32, 1, 254, 255] {
            for max_width in [0u32, 1, 254, 255] {
                let mut copy = img.clone();
                let removed = remove_thin_pockets_in(
                    &mut copy,
                    w,
                    h,
                    [250, 250, 250],
                    10,
                    reach,
                    max_width,
                    &mut scratch,
                );
                assert!(removed <= w * h, "reach {reach}, max_width {max_width}");
                assert_eq!(copy.len(), w * h * 4);
            }
        }

        for depth in [0u32, 1, 2, 254, 255] {
            let mut copy = img.clone();
            let changed = defringe_in(&mut copy, w, h, [250, 250, 250], depth, &mut scratch);
            assert!(changed <= w * h, "depth {depth}");
        }
    }
}
