//! Проверка, что «горячий» путь кадра не аллоцирует.
//!
//! Отдельный тестовый бинарь со своим считающим аллокатором: глобальный
//! аллокатор видит только этот тест, поэтому счётчик не размывается другими.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use pixel_image_gui::model::Model;
use pixel_image_gui::settings::Settings;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn count<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCS.load(Relaxed);
    let out = f();
    (out, ALLOCS.load(Relaxed) - before)
}

/// Отпечаток состояния для сохранения настроек считается без аллокаций:
/// это то, что кадр делает на статичном UI вместо `Settings::from_model`.
#[test]
fn settings_signature_does_not_allocate() {
    let mut model = Model::new();
    model.output.prefix = Some("префикс".to_string());
    model
        .recent
        .push(std::path::PathBuf::from("/tmp/каталог/файл.png"));

    let window = Some((1360u32, 820u32));

    // Прогрев: пусть первый вызов (если что-то лениво инициализируется)
    // случится до замера.
    let _ = Settings::signature(&model, window, false);

    let (signature, allocs) = count(|| Settings::signature(&model, window, false));
    assert_eq!(allocs, 0, "отпечаток настроек аллоцировал {allocs} раз");
    assert_ne!(signature, 0, "отпечаток не должен быть вырожденным");

    // Для сравнения: полный документ аллоцирует — именно его кадр и перестал
    // строить.
    let (_, document_allocs) = count(|| Settings::from_model(&model, window, false));
    assert!(
        document_allocs > 0,
        "ожидалось, что from_model аллоцирует (получено {document_allocs})"
    );
}
