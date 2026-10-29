use crate::Launcher;
use gpui::{
    profiler::{FrameEvent, FrameTimingCollector},
    *,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

fn summary(mut values: Vec<f64>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(f64::total_cmp);
    json!({"samples": values.len(), "mean": values.iter().sum::<f64>() / values.len() as f64,
        "p50": values[(values.len() - 1) / 2], "p95": values[(values.len() - 1) * 95 / 100],
        "p99": values[(values.len() - 1) * 99 / 100], "max": values.last()})
}

pub fn start(window: &Window, cx: &mut Context<Launcher>) {
    let Ok(output) = std::env::var("RAESIR_BENCH_OUTPUT") else {
        return;
    };
    let output = PathBuf::from(output);
    gpui::profiler::set_trace_enabled(true);
    let fast = std::env::var("RAESIR_BENCH_INPUT").is_ok_and(|v| v == "spin");
    let step = if fast { 4 } else { 60 };
    let lines = if fast { 0.2 } else { 3. };
    cx.spawn_in(window, async move |this, cx| {
        fs::create_dir_all(&output).unwrap();
        cx.background_executor().timer(millis(2500)).await;
        let mut measured = vec![];
        let mut refresh = None;
        let mut active = false;
        for (name, duration) in [("warmup", 4000), ("measure", 8000)] {
            this.update_in(cx, |this, window, cx| {
                this.rows.set_offset_from_scrollbar(point(px(0.), px(0.)));
                this.wheel.reset(0.);
                this.wheel_needs_sync = false;
                refresh = window.refresh_interval(cx).map(|v| 1. / v.as_secs_f64());
                active = window.is_window_active();
                cx.notify();
            }).unwrap();
            cx.background_executor().timer(millis(350)).await;
            eprintln!("BENCH {name}, active={active}, refresh={refresh:?}");
            let mut collector = FrameTimingCollector::new();
            let start = Instant::now();
            for tick in 0..duration / step {
                let deadline = start + millis(tick * step);
                cx.background_executor().timer(deadline.saturating_duration_since(Instant::now())).await;
                this.update_in(cx, |this, window, cx| {
                    let direction = if (tick * step / 2000) % 2 == 0 { -1. } else { 1. };
                    this.scroll(&ScrollWheelEvent {
                        position: point(px(0.), px(0.)), delta: ScrollDelta::Lines(point(0., lines * direction)),
                        modifiers: Modifiers::default(), touch_phase: TouchPhase::Moved,
                    }, window, cx);
                }).unwrap();
                if name == "measure" { measured.extend(collector.collect_unseen()); }
            }
            cx.background_executor().timer(millis(350)).await;
            if name == "measure" { measured.extend(collector.collect_unseen()); }
        }
        let mut draw = vec![];
        let mut layout = vec![];
        let mut prepaint = vec![];
        let mut paint = vec![];
        let mut submit = vec![];
        let mut present = vec![];
        let mut csv = BufWriter::new(fs::File::create(output.join("frames.csv")).unwrap());
        writeln!(csv, "event,time_ms,draw_ms,layout_ms,prepaint_ms,paint_ms,submit_ms").unwrap();
        let base = measured.iter().find_map(|e| match e { FrameEvent::Draw(t) => Some(t.draw_start), _ => None }).unwrap();
        for event in &measured {
            match event {
                FrameEvent::Draw(t) => {
                    let values = [t.draw_duration(), t.phases.request_layout, t.phases.prepaint, t.phases.paint].map(|v| v.as_secs_f64() * 1000.);
                    draw.push(values[0]); layout.push(values[1]); prepaint.push(values[2]); paint.push(values[3]);
                    writeln!(csv, "draw,{:.4},{:.4},{:.4},{:.4},{:.4},", t.draw_start.duration_since(base).as_secs_f64() * 1000., values[0], values[1], values[2], values[3]).unwrap();
                }
                FrameEvent::Present(t) => {
                    let elapsed = t.present_end.duration_since(base).as_secs_f64() * 1000.;
                    let duration = t.present_duration().as_secs_f64() * 1000.;
                    submit.push(duration); present.push(elapsed);
                    writeln!(csv, "present,{elapsed:.4},,,,,{duration:.4}").unwrap();
                }
            }
        }
        let intervals: Vec<_> = present.windows(2).map(|v| v[1] - v[0]).collect();
        let fps = (present.len() - 1) as f64 * 1000. / (present.last().unwrap() - present[0]);
        let report = json!({"blur": std::env::var_os("RAESIR_MOCK_NO_BLUR").is_none(),
            "inactive_cap": std::env::var_os("RAESIR_MOCK_LIMIT_BACKGROUND").is_some(),
            "active": active,
            "input": if fast { "spin" } else { "wheel" }, "refresh_hz": refresh, "submitted_fps": fps,
            "present_interval_ms": summary(intervals), "cpu_draw_ms": summary(draw), "layout_ms": summary(layout),
            "prepaint_ms": summary(prepaint), "paint_ms": summary(paint), "submit_ms": summary(submit)});
        fs::write(output.join("summary.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
        eprintln!("BENCH done: {report}");
        cx.update(|_, cx| cx.quit()).ok();
    }).detach();
}
