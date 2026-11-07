//! Параллель: один пул rayon на процесс и порог, ниже которого она не нужна.
//!
//! Пул не свой: берём глобальный пул rayon. Он ровно один на процесс, создаётся
//! лениво по первому обращению, размер по умолчанию — `available_parallelism()`,
//! а `RAYON_NUM_THREADS` учитывается самим rayon (значение `1` даёт
//! последовательный режим — на этом держатся тесты О-2 и `bench.ps1`).
//!
//! Так сделано намеренно вместо собственного `ThreadPool` + `install()` из
//! плана: свой пул пришлось бы вызывать из каждой функции, а `install` внутрь
//! пула, который уже работает на тесте, может встать в дедлок (rayon не
//! поддерживает вложенные пулы). С глобальным пулом достаточно того, что тест
//! сам оборачивает вызов в `ThreadPool::install`, и параллельные итераторы
//! внутри пойдут ровно по этому пулу.
//!
//! Ядро само пул не создаёт: [`init`] вызывают хосты (GUI-воркер, CLI), чтобы
//! первый тяжёлый кадр не платил за запуск потоков.

/// Минимальный объём работы в пикселях, при котором параллель окупается.
///
/// Ниже порога проход идёт последовательно: у мелких буферов (превью сетки
/// 128×85, тестовые фикстуры 256×256) накладные расходы rayon больше выигрыша,
/// и, что важнее, тесты бюджета аллокаций не должны видеть чужие аллокации.
pub const MIN_PIXELS: usize = 1 << 18;

/// То же в байтах RGBA.
const MIN_BYTES: usize = MIN_PIXELS * 4;

/// Сколько потоков видит rayon прямо сейчас.
pub fn num_threads() -> usize {
    rayon::current_num_threads()
}

/// Стоит ли распараллеливать работу объёмом `pixels` пикселей.
#[inline]
pub fn should_parallel(pixels: usize) -> bool {
    pixels >= MIN_PIXELS && num_threads() > 1
}

/// То же для сырого RGBA-буфера.
#[inline]
pub fn should_parallel_bytes(bytes: usize) -> bool {
    bytes >= MIN_BYTES && num_threads() > 1
}

/// Создаёт общий пул заранее, чтобы первый тяжёлый кадр не ждал потоков.
///
/// Повторный вызов безопасен: `build_global` возвращает ошибку, если пул уже
/// инициализирован, и мы её игнорируем. `RAYON_NUM_THREADS` по-прежнему в силе.
pub fn init() {
    let _ = rayon::ThreadPoolBuilder::new().build_global();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пул один на процесс: размер не «плывёт» между вызовами, а порог
    /// включения согласован с числом потоков.
    #[test]
    fn pool_is_singleton_and_threshold_matches_threads() {
        init();
        init(); // повторная инициализация не должна паниковать
        let threads = num_threads();
        assert!(threads >= 1, "потоков {threads}");

        assert!(!should_parallel(0));
        assert!(!should_parallel(MIN_PIXELS - 1));
        assert_eq!(should_parallel(MIN_PIXELS), threads > 1);
        assert!(!should_parallel_bytes(MIN_BYTES - 4));
        assert_eq!(should_parallel_bytes(MIN_BYTES), threads > 1);
        assert_eq!(num_threads(), threads, "размер пула не меняется");
    }
}
