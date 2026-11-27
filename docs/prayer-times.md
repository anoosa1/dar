# Prayer Times

Prayer Times is a dependency-free Rust module inside Dar's browser client (`crates/client/src/prayers/`) and runtime (`crates/runtime/src/prayers.rs`). It computes prayer times, Qibla bearings, solar alignment cues, solar/night landmarks and tabular Hijri dates 100% offline with zero external API, IP lookup or geocoding calls, and stores settings and yearly daily prayer logs per account with optimistic `If-Match` revision checks.

## Astronomical model

- **Solar coordinates:** US Naval Observatory (USNO) approximate apparent solar longitude, obliquity, right ascension and equation of time evaluated dynamically at each candidate instant.
- **Event root-finding:** Brackets and bisects the geometric solar altitude to sub-second precision before rounding to the nearest minute after applying user offsets.
- **Sunrise and sunset:** USNO sea-level `-0.833333°` solar altitude convention (unobstructed sea-level horizon with standard refraction; no terrain or weather modeling).
- **Qibla:** Initial spherical great-circle bearing from true north and Haversine distance to the Kaaba (`21.422487° N, 39.826206° E`), paired with the current solar azimuth to offer a live solar alignment cue.
- **Hijri date:** Civil tabular Hijri (Kuwaiti algorithm) with an explicit `±2` day sighting offset.

## Supported methods & conventions

- **Methods:** `MWL` (`18°`/`17°`), `ISNA` (`15°`/`15°`), `Egypt` (`19.5°`/`17.5°`), `Karachi` (`18°`/`18°`), `UmmAlQura` (`18.5°`/`+90 min`), `Diyanet` (`18°`/`17°`), `MUIS` (`20°`/`18°`), `UOIF` (`12°`/`12°`), `Tehran` (`17.7°`/`14°`) and `Custom` (configurable Fajr/Isha angles or fixed interval after Maghrib).
- **Asr shadow rule:** `standard` (`1×` object shadow + noon shadow) or `hanafi` (`2×` object shadow + noon shadow).
- **High-latitude bounds:** `none`, `middle` (1/2 of actual adjacent sunset-to-sunrise night), `seventh` (1/7), or `angle` (`angle / 60`). Polar events without a solar crossing stay explicitly `Unavailable` rather than fabricated.
- **Date range & timezones:** Gregorian civil dates `2000–2099`, validated against explicit IANA timezones (`chrono_tz` on the server, `Intl.DateTimeFormat` in the browser). Browser geolocation runs only on explicit user action.

## Daily prayer tracking & storage

- **Obligatory prayers tracked daily:** `fajr`, `dhuhr`, `asr`, `maghrib`, `isha` (`sunrise` is shown as a solar landmark and never tracked).
- **States:** `unprayed` (*Not prayed yet*), `on_time` (*Prayed on time*), and `late` (*Prayed late*), displayed with morphing status icons (`Not yet` / `On time` / `Late` in tooltips and accessible labels).
- **Persistence:** Stored in SpacetimeDB's per-user `app_data` table under `org.dar.prayer-times` across `settings` (preserving existing settings transparently) and yearly log records `log-YYYY` (`YYYY-MM-DD -> DayLog`, with all-unprayed days pruned), inheriting Dar's 64 KiB/record × 128-record (1 MiB) quota, backup/restore, and account deletion.
- **API:** `/api/v1/prayers/settings` (`GET`/`PUT`/`DELETE`), `/api/v1/prayers/log?year=YYYY` (`GET`/`PUT`), and `/api/v1/prayers/log/{date}` (`PATCH`), disabled with HTTP `404` when an administrator turns Prayer Times off.
