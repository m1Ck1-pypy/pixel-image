mod args;

use std::{process, time::Instant};

use args::Args;
use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
use pixel_image_core::{CleanStats, StageTimes, convert_file, cutout_file};

fn main() {
    let matches = Args::command().get_matches();

    // Режимы жёстко разделены: пиксельные опции несовместимы с --cutbg.
    if matches.get_flag("cutbg") {
        const PIXEL_FLAGS: [(&str, &str); 9] = [
            ("--width", "width"),
            ("--filter", "filter"),
            ("--colors", "colors"),
            ("--cell", "cell"),
            ("--grid-opacity", "grid_opacity"),
            ("--grid-color", "grid_color"),
            ("--alpha-threshold", "alpha_threshold"),
            ("--background", "background"),
            ("--despeckle", "despeckle"),
        ];
        for (flag, id) in PIXEL_FLAGS {
            if matches.value_source(id) == Some(ValueSource::CommandLine) {
                eprintln!(
                    "Ошибка: {flag} несовместим с режимом --cutbg: вырезка только удаляет фон,\nпиксель-арт при этом не создаётся."
                );
                eprintln!("Пример: pixel-image photo.jpg --cutbg");
                eprintln!("Пример: pixel-image photo.jpg --cutbg --remove-holes --bg-grow 1");
                process::exit(2);
            }
        }
    }
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // Опции уточнения фона требуют его удаления: --remove-bg или --cutbg.
    if (args.bg_color.is_some()
        || args.remove_holes
        || args.bg_defringe.is_some()
        || args.thin_gaps)
        && !args.removes_background()
    {
        eprintln!(
            "Ошибка: --bg-color, --remove-holes, --bg-defringe и --thin-gaps требуют --remove-bg или --cutbg."
        );
        eprintln!("Пример: pixel-image photo.jpg --remove-bg --remove-holes");
        eprintln!("Пример: pixel-image photo.jpg --remove-bg --bg-defringe --thin-gaps");
        eprintln!("Пример: pixel-image photo.jpg --cutbg --bg-color ffffff");
        process::exit(2);
    }

    let result = if args.cutbg {
        run_cutbg(&args)
    } else {
        run(&args)
    };
    if let Err(e) = result {
        eprintln!("Ошибка: {e}");
        process::exit(1);
    }
}

/// Печатает время по этапам (`--verbose`). Нулевые этапы не показываются:
/// у `--cutbg` нет уменьшения и сетки, у пиксель-арта — своей очистки фона.
fn print_stages(times: &StageTimes) {
    let mut parts: Vec<String> = Vec::with_capacity(9);
    let stages = [
        ("открытие", times.open),
        ("очистка", times.clean),
        ("уменьшение", times.downscale),
        ("порог+мусор", times.threshold),
        ("гистограмма", times.histogram),
        ("палитра", times.palette),
        ("ремап", times.remap),
        ("сетка", times.upscale),
        ("запись", times.save),
    ];
    for (name, ms) in stages {
        if ms > 0.0 {
            parts.push(format!("{name} {ms:.1} мс"));
        }
    }
    println!("Этапы: {} (сумма {:.1} мс)", parts.join(", "), times.total());
}

/// Печатает статистику очистки исходника в том же порядке, в котором она
/// собиралась.
fn print_clean_stats(stats: &CleanStats, thin_gaps_enabled: bool) {
    if stats.faint_alpha_removed > 0 {
        println!(
            "Убрано слабопрозрачных пикселей: {}",
            stats.faint_alpha_removed
        );
    }
    let Some(bg) = &stats.background else {
        return;
    };
    if bg.defringe_depth > 0 {
        println!(
            "Матирование края (полоса {} px): изменено {} px, тонких щелей удалено {} px",
            bg.defringe_depth, bg.defringe_changed, bg.thin_gaps_removed
        );
    } else if thin_gaps_enabled {
        println!(
            "Тонких щелей цвета фона удалено: {} px",
            bg.thin_gaps_removed
        );
    }
    println!(
        "Фон #{:02x}{:02x}{:02x}: удалено {} px ({:.1}%)",
        bg.color[0], bg.color[1], bg.color[2], bg.removed, bg.removed_percent
    );
}

/// Режим вырезки: исходное изображение в исходном размере, но с прозрачным фоном.
fn run_cutbg(args: &Args) -> pixel_image_core::Result<()> {
    let t0 = Instant::now();
    let background = args.background_options();
    let saved = cutout_file(&args.input, &background, args.prefix.as_deref())?;

    print_clean_stats(&saved.stats, args.thin_gaps);
    if saved.fully_transparent {
        println!("Предупреждение: объект не найден — вырезка полностью прозрачна");
    }

    println!(
        "Вырезка ({}x{}): {}",
        saved.width,
        saved.height,
        saved.path.display()
    );
    if args.verbose {
        print_stages(&saved.times);
    }
    println!("Время: {:.2?}", t0.elapsed());
    Ok(())
}

fn run(args: &Args) -> pixel_image_core::Result<()> {
    let t0 = Instant::now();
    let options = args.options();
    let saved = convert_file(&args.input, &options, args.prefix.as_deref())?;
    let stats = &saved.stats;

    print_clean_stats(&stats.clean, args.thin_gaps);
    println!(
        "Сетка {}x{}, фильтр {:?}, уникальных цветов {} -> палитра {}",
        stats.grid_width,
        stats.grid_height,
        options.filter,
        stats.unique_colors,
        stats.palette_colors
    );
    if stats.transparent_cells > 0 {
        let how = if options.fill.is_some() {
            "залиты фоном"
        } else {
            "сохранены прозрачными"
        };
        println!(
            "Прозрачных ячеек: {} из {} ({how})",
            stats.transparent_cells, stats.total_cells
        );
    }
    println!(
        "С сеткой ({}x{}): {}",
        stats.output_width,
        stats.output_height,
        saved.grid_path.display()
    );
    println!(
        "Спрайт ({}x{}): {}",
        stats.grid_width,
        stats.grid_height,
        saved.sprite_path.display()
    );
    if args.verbose {
        print_stages(&saved.times);
    }
    println!("Время: {:.2?}", t0.elapsed());
    Ok(())
}
