//! Cached `Intl.DateTimeFormat` formatting (`client/dates.ts`): the same output as
//! `toLocaleDateString` and friends, including the fields they fill in by default.

use std::cell::RefCell;
use std::collections::HashMap;

use js_sys::{Array, Date, Intl, Object, Reflect};
use wasm_bindgen::JsValue;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Date,
    Time,
    Any,
}

thread_local! {
    static FORMATS: RefCell<HashMap<String, Intl::DateTimeFormat>> = RefCell::new(HashMap::new());
}

const DATE_FIELDS: [&str; 4] = ["weekday", "year", "month", "day"];
const TIME_FIELDS: [&str; 5] = [
    "dayPeriod",
    "hour",
    "minute",
    "second",
    "fractionalSecondDigits",
];

fn format(value: f64, options: &[(&str, &str)], kind: Kind) -> String {
    let key = format!("{}{:?}", kind as u8, options);
    FORMATS.with(|formats| {
        let mut formats = formats.borrow_mut();
        let formatter = formats.entry(key).or_insert_with(|| {
            let has = |fields: &[&str]| options.iter().any(|(name, _)| fields.contains(name));
            let styled = options
                .iter()
                .any(|(name, _)| *name == "dateStyle" || *name == "timeStyle");
            let bare = !styled
                && match kind {
                    Kind::Date => !has(&DATE_FIELDS),
                    Kind::Time => !has(&TIME_FIELDS),
                    Kind::Any => !has(&DATE_FIELDS) && !has(&TIME_FIELDS),
                };
            let object = Object::new();
            for (name, value) in options {
                let _ = Reflect::set(&object, &JsValue::from_str(name), &JsValue::from_str(value));
            }
            if bare && kind != Kind::Time {
                for name in ["year", "month", "day"] {
                    let _ = Reflect::set(
                        &object,
                        &JsValue::from_str(name),
                        &JsValue::from_str("numeric"),
                    );
                }
            }
            if bare && kind != Kind::Date {
                for name in ["hour", "minute", "second"] {
                    let _ = Reflect::set(
                        &object,
                        &JsValue::from_str(name),
                        &JsValue::from_str("numeric"),
                    );
                }
            }
            Intl::DateTimeFormat::new(&Array::new(), &object)
        });
        let date = Date::new(&JsValue::from_f64(value));
        formatter
            .format()
            .call1(&JsValue::NULL, &date)
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default()
    })
}

/// Like `new Date(value).toLocaleDateString(undefined, options)`.
pub fn local_date(value: f64, options: &[(&str, &str)]) -> String {
    format(value, options, Kind::Date)
}

/// Like `toLocaleTimeString(undefined, options)`.
pub fn local_time(value: f64, options: &[(&str, &str)]) -> String {
    format(value, options, Kind::Time)
}

/// Like `toLocaleString(undefined, options)`.
pub fn local_date_time(value: f64, options: &[(&str, &str)]) -> String {
    format(value, options, Kind::Any)
}

fn start_of_day(value: f64) -> f64 {
    let date = Date::new(&JsValue::from_f64(value));
    Date::new_with_year_month_day(
        date.get_full_year(),
        date.get_month() as i32,
        date.get_date() as i32,
    )
    .get_time()
}

/// Whole calendar days from `value` to now, in local time.
pub fn days_ago(value: f64) -> i64 {
    ((start_of_day(Date::now()) - start_of_day(value)) / 86_400_000.0).round() as i64
}

/// "Today, 3:04 PM", "Yesterday", then "Nov 18" (Files' `relative`).
pub fn relative(value: f64) -> String {
    match days_ago(value) {
        0 => format!(
            "Today, {}",
            local_time(value, &[("hour", "numeric"), ("minute", "2-digit")])
        ),
        1 => "Yesterday".into(),
        _ => short_date(value),
    }
}

/// "Nov 18" (Files' `date`).
pub fn short_date(value: f64) -> String {
    local_date(value, &[("month", "short"), ("day", "numeric")])
}

/// A short edit time: the time today, Yesterday, then the date (`pim/presentation.ts` `edited`).
pub fn edited(value: f64) -> String {
    match days_ago(value) {
        0 => local_time(value, &[("hour", "numeric"), ("minute", "2-digit")]),
        1 => "Yesterday".into(),
        _ => {
            let same_year = Date::new(&JsValue::from_f64(value)).get_full_year()
                == Date::new_0().get_full_year();
            if same_year {
                local_date(value, &[("month", "short"), ("day", "numeric")])
            } else {
                local_date(
                    value,
                    &[("month", "short"), ("day", "numeric"), ("year", "numeric")],
                )
            }
        }
    }
}
