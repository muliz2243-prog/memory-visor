use egui::Color32;

/// Every colour the renderer may use. Dark surface, bright alerts: the failing
/// cell has to stand out even in a screenshot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub background: Color32,
    pub panel_fill: Color32,
    pub frame_fill: Color32,
    pub frame_header: Color32,
    pub object_fill: Color32,
    pub object_header: Color32,
    pub released_fill: Color32,
    pub released_header: Color32,
    pub cell_fill: Color32,
    pub stroke: Color32,
    pub alert_fill: Color32,
    pub alert_stroke: Color32,
    pub alert_text: Color32,
    pub effect_fill: Color32,
    pub effect_stroke: Color32,
    pub effect_text: Color32,
    pub code_active_fill: Color32,
    pub code_active_stroke: Color32,
    pub arrow: Color32,
    pub text: Color32,
    pub title_text: Color32,
    pub muted_text: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Palette {
            background: Color32::from_rgb(16, 18, 23),
            panel_fill: Color32::from_rgb(22, 25, 31),
            frame_fill: Color32::from_rgb(30, 34, 42),
            frame_header: Color32::from_rgb(48, 56, 70),
            object_fill: Color32::from_rgb(28, 40, 38),
            object_header: Color32::from_rgb(40, 62, 58),
            released_fill: Color32::from_rgb(28, 29, 32),
            released_header: Color32::from_rgb(44, 45, 50),
            cell_fill: Color32::from_rgb(24, 27, 33),
            stroke: Color32::from_rgb(78, 86, 100),
            alert_fill: Color32::from_rgb(70, 20, 20),
            alert_stroke: Color32::from_rgb(255, 70, 70),
            alert_text: Color32::from_rgb(255, 214, 214),
            effect_fill: Color32::from_rgb(72, 58, 20),
            effect_stroke: Color32::from_rgb(255, 196, 87),
            effect_text: Color32::from_rgb(255, 236, 190),
            code_active_fill: Color32::from_rgb(34, 46, 62),
            code_active_stroke: Color32::from_rgb(110, 170, 255),
            arrow: Color32::from_rgb(110, 170, 255),
            text: Color32::from_rgb(222, 226, 233),
            title_text: Color32::from_rgb(245, 247, 250),
            muted_text: Color32::from_rgb(140, 146, 158),
        }
    }
}

/// Every word the panels print. `format` uses the two reference states; the
/// rest is chrome that only the renderer needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Texts {
    pub stack_title: String,
    pub heap_title: String,
    pub line_label: String,
    pub released_object: String,
    pub missing_object: String,
    pub unknown_value: String,
    pub empty_stack: String,
    pub empty_heap: String,
    pub step_label: String,
    pub incident_label: String,
    pub play_label: String,
    pub pause_label: String,
    pub trace_title: String,
    pub trace_hint: String,
    pub start_label: String,
    pub code_title: String,
    pub code_hint: String,
    pub recorded_label: String,
}

impl Texts {
    pub fn english() -> Self {
        Texts {
            stack_title: "Stack".to_string(),
            heap_title: "Heap".to_string(),
            line_label: "line".to_string(),
            released_object: "released".to_string(),
            missing_object: "no object".to_string(),
            unknown_value: "?".to_string(),
            empty_stack: "(empty stack)".to_string(),
            empty_heap: "(empty heap)".to_string(),
            step_label: "step".to_string(),
            incident_label: "incident".to_string(),
            play_label: "Play".to_string(),
            pause_label: "Pause".to_string(),
            trace_title: "Trace".to_string(),
            trace_hint: "click a row to jump".to_string(),
            start_label: "start".to_string(),
            code_title: "Source".to_string(),
            code_hint: "click a line to jump".to_string(),
            recorded_label: "Recorded".to_string(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Theme {
    pub palette: Palette,
    pub texts: Texts,
}

impl Theme {
    pub fn dark(texts: Texts) -> Self {
        Theme {
            palette: Palette::dark(),
            texts,
        }
    }

    pub fn english() -> Self {
        Theme::dark(Texts::english())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::english()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alert_colour_is_far_from_the_panel_fill() {
        let palette = Palette::dark();
        let alert = palette.alert_stroke;
        assert!(alert.r() > 200);
        assert!(alert.r() > alert.g() + 100);
        assert!(alert.r() > alert.b() + 100);
        assert_ne!(palette.alert_fill, palette.cell_fill);
        assert_ne!(palette.effect_stroke, palette.alert_stroke);
        assert_ne!(palette.effect_fill, palette.alert_fill);
        assert_ne!(palette.code_active_fill, palette.background);
        assert_ne!(palette.code_active_stroke, palette.alert_stroke);
    }

    #[test]
    fn the_dictionary_has_no_empty_entries() {
        let english = Texts::english();
        assert_eq!(english.released_object, "released");
        for text in [
            &english.stack_title,
            &english.heap_title,
            &english.line_label,
            &english.missing_object,
            &english.unknown_value,
            &english.empty_stack,
            &english.empty_heap,
            &english.step_label,
            &english.incident_label,
            &english.play_label,
            &english.pause_label,
            &english.trace_title,
            &english.trace_hint,
            &english.start_label,
            &english.code_title,
            &english.code_hint,
            &english.recorded_label,
        ] {
            assert!(!text.is_empty());
        }
    }
}
