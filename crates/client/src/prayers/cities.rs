//! Offline world city catalog (~60 major cities) with coordinates, IANA timezone,
//! and regionally customary calculation method.

use crate::prayers::model::Location;

#[derive(Clone, Copy, Debug)]
pub struct City {
    pub name: &'static str,
    pub country: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub tz: &'static str,
    pub recommended_method: &'static str,
}

impl City {
    pub fn to_location(self) -> Location {
        Location {
            label: self.name.to_owned(),
            latitude: self.lat,
            longitude: self.lon,
            timezone: self.tz.to_owned(),
        }
    }
}

pub const QUICK_CITIES: &[&str] = &[
    "Toronto",
    "London",
    "Makkah",
    "Istanbul",
    "New York",
    "Dubai",
    "Kuala Lumpur",
    "Cairo",
];

pub const CITIES: &[City] = &[
    City { name: "Toronto", country: "Canada", lat: 43.6532, lon: -79.3832, tz: "America/Toronto", recommended_method: "ISNA" },
    City { name: "London", country: "United Kingdom", lat: 51.5074, lon: -0.1278, tz: "Europe/London", recommended_method: "MWL" },
    City { name: "Makkah", country: "Saudi Arabia", lat: 21.4225, lon: 39.8262, tz: "Asia/Riyadh", recommended_method: "UmmAlQura" },
    City { name: "Madinah", country: "Saudi Arabia", lat: 24.4672, lon: 39.6111, tz: "Asia/Riyadh", recommended_method: "UmmAlQura" },
    City { name: "Istanbul", country: "Turkey", lat: 41.0082, lon: 28.9784, tz: "Europe/Istanbul", recommended_method: "Diyanet" },
    City { name: "New York", country: "United States", lat: 40.7128, lon: -74.0060, tz: "America/New_York", recommended_method: "ISNA" },
    City { name: "Dubai", country: "UAE", lat: 25.2048, lon: 55.2708, tz: "Asia/Dubai", recommended_method: "UmmAlQura" },
    City { name: "Kuala Lumpur", country: "Malaysia", lat: 3.1390, lon: 101.6869, tz: "Asia/Kuala_Lumpur", recommended_method: "MUIS" },
    City { name: "Cairo", country: "Egypt", lat: 30.0444, lon: 31.2357, tz: "Africa/Cairo", recommended_method: "Egypt" },
    City { name: "Riyadh", country: "Saudi Arabia", lat: 24.7136, lon: 46.6753, tz: "Asia/Riyadh", recommended_method: "UmmAlQura" },
    City { name: "Jeddah", country: "Saudi Arabia", lat: 21.4858, lon: 39.1925, tz: "Asia/Riyadh", recommended_method: "UmmAlQura" },
    City { name: "Doha", country: "Qatar", lat: 25.2854, lon: 51.5310, tz: "Asia/Qatar", recommended_method: "UmmAlQura" },
    City { name: "Abu Dhabi", country: "UAE", lat: 24.4539, lon: 54.3773, tz: "Asia/Dubai", recommended_method: "UmmAlQura" },
    City { name: "Kuwait City", country: "Kuwait", lat: 29.3759, lon: 47.9774, tz: "Asia/Kuwait", recommended_method: "UmmAlQura" },
    City { name: "Muscat", country: "Oman", lat: 23.5880, lon: 58.3829, tz: "Asia/Muscat", recommended_method: "UmmAlQura" },
    City { name: "Amman", country: "Jordan", lat: 31.9454, lon: 35.9284, tz: "Asia/Amman", recommended_method: "Egypt" },
    City { name: "Jerusalem", country: "Palestine", lat: 31.7683, lon: 35.2137, tz: "Asia/Jerusalem", recommended_method: "Egypt" },
    City { name: "Beirut", country: "Lebanon", lat: 33.8938, lon: 35.5018, tz: "Asia/Beirut", recommended_method: "Egypt" },
    City { name: "Ankara", country: "Turkey", lat: 39.9334, lon: 32.8597, tz: "Europe/Istanbul", recommended_method: "Diyanet" },
    City { name: "Tehran", country: "Iran", lat: 35.6892, lon: 51.3890, tz: "Asia/Tehran", recommended_method: "Tehran" },
    City { name: "Baghdad", country: "Iraq", lat: 33.3152, lon: 44.3661, tz: "Asia/Baghdad", recommended_method: "MWL" },
    City { name: "Karachi", country: "Pakistan", lat: 24.8607, lon: 67.0011, tz: "Asia/Karachi", recommended_method: "Karachi" },
    City { name: "Lahore", country: "Pakistan", lat: 31.5204, lon: 74.3587, tz: "Asia/Karachi", recommended_method: "Karachi" },
    City { name: "Islamabad", country: "Pakistan", lat: 33.6844, lon: 73.0479, tz: "Asia/Karachi", recommended_method: "Karachi" },
    City { name: "Dhaka", country: "Bangladesh", lat: 23.8103, lon: 90.4125, tz: "Asia/Dhaka", recommended_method: "Karachi" },
    City { name: "New Delhi", country: "India", lat: 28.6139, lon: 77.2090, tz: "Asia/Kolkata", recommended_method: "Karachi" },
    City { name: "Mumbai", country: "India", lat: 19.0760, lon: 72.8777, tz: "Asia/Kolkata", recommended_method: "Karachi" },
    City { name: "Hyderabad", country: "India", lat: 17.3850, lon: 78.4867, tz: "Asia/Kolkata", recommended_method: "Karachi" },
    City { name: "Singapore", country: "Singapore", lat: 1.3521, lon: 103.8198, tz: "Asia/Singapore", recommended_method: "MUIS" },
    City { name: "Jakarta", country: "Indonesia", lat: -6.2088, lon: 106.8456, tz: "Asia/Jakarta", recommended_method: "MUIS" },
    City { name: "Tokyo", country: "Japan", lat: 35.6762, lon: 139.6503, tz: "Asia/Tokyo", recommended_method: "MWL" },
    City { name: "Paris", country: "France", lat: 48.8566, lon: 2.3522, tz: "Europe/Paris", recommended_method: "UOIF" },
    City { name: "Berlin", country: "Germany", lat: 52.5200, lon: 13.4050, tz: "Europe/Berlin", recommended_method: "MWL" },
    City { name: "Amsterdam", country: "Netherlands", lat: 52.3676, lon: 4.9041, tz: "Europe/Amsterdam", recommended_method: "MWL" },
    City { name: "Brussels", country: "Belgium", lat: 50.8503, lon: 4.3517, tz: "Europe/Brussels", recommended_method: "UOIF" },
    City { name: "Madrid", country: "Spain", lat: 40.4168, lon: -3.7038, tz: "Europe/Madrid", recommended_method: "MWL" },
    City { name: "Rome", country: "Italy", lat: 41.9028, lon: 12.4964, tz: "Europe/Rome", recommended_method: "MWL" },
    City { name: "Oslo", country: "Norway", lat: 59.9139, lon: 10.7522, tz: "Europe/Oslo", recommended_method: "MWL" },
    City { name: "Stockholm", country: "Sweden", lat: 59.3293, lon: 18.0686, tz: "Europe/Stockholm", recommended_method: "MWL" },
    City { name: "Birmingham", country: "United Kingdom", lat: 52.4862, lon: -1.8904, tz: "Europe/London", recommended_method: "MWL" },
    City { name: "Manchester", country: "United Kingdom", lat: 53.4808, lon: -2.2426, tz: "Europe/London", recommended_method: "MWL" },
    City { name: "Casablanca", country: "Morocco", lat: 33.5731, lon: -7.5898, tz: "Africa/Casablanca", recommended_method: "MWL" },
    City { name: "Tunis", country: "Tunisia", lat: 36.8065, lon: 10.1815, tz: "Africa/Tunis", recommended_method: "MWL" },
    City { name: "Algiers", country: "Algeria", lat: 36.7538, lon: 3.0588, tz: "Africa/Algiers", recommended_method: "MWL" },
    City { name: "Lagos", country: "Nigeria", lat: 6.5244, lon: 3.3792, tz: "Africa/Lagos", recommended_method: "Egypt" },
    City { name: "Nairobi", country: "Kenya", lat: -1.2921, lon: 36.8219, tz: "Africa/Nairobi", recommended_method: "Egypt" },
    City { name: "Johannesburg", country: "South Africa", lat: -26.2041, lon: 28.0473, tz: "Africa/Johannesburg", recommended_method: "MWL" },
    City { name: "Montreal", country: "Canada", lat: 45.5017, lon: -73.5673, tz: "America/Toronto", recommended_method: "ISNA" },
    City { name: "Ottawa", country: "Canada", lat: 45.4215, lon: -75.6972, tz: "America/Toronto", recommended_method: "ISNA" },
    City { name: "Vancouver", country: "Canada", lat: 49.2827, lon: -123.1207, tz: "America/Vancouver", recommended_method: "ISNA" },
    City { name: "Calgary", country: "Canada", lat: 51.0447, lon: -114.0719, tz: "America/Edmonton", recommended_method: "ISNA" },
    City { name: "Chicago", country: "United States", lat: 41.8781, lon: -87.6298, tz: "America/Chicago", recommended_method: "ISNA" },
    City { name: "Detroit", country: "United States", lat: 42.3314, lon: -83.0458, tz: "America/Detroit", recommended_method: "ISNA" },
    City { name: "Houston", country: "United States", lat: 29.7604, lon: -95.3698, tz: "America/Chicago", recommended_method: "ISNA" },
    City { name: "Los Angeles", country: "United States", lat: 34.0522, lon: -118.2437, tz: "America/Los_Angeles", recommended_method: "ISNA" },
    City { name: "San Francisco", country: "United States", lat: 37.7749, lon: -122.4194, tz: "America/Los_Angeles", recommended_method: "ISNA" },
    City { name: "Washington, D.C.", country: "United States", lat: 38.9072, lon: -77.0369, tz: "America/New_York", recommended_method: "ISNA" },
    City { name: "Sydney", country: "Australia", lat: -33.8688, lon: 151.2093, tz: "Australia/Sydney", recommended_method: "MWL" },
    City { name: "Melbourne", country: "Australia", lat: -37.8136, lon: 144.9631, tz: "Australia/Melbourne", recommended_method: "MWL" },
];

pub fn find_city(name: &str) -> Option<&'static City> {
    CITIES.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

pub fn search_cities(query: &str, limit: usize) -> Vec<&'static City> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return CITIES.iter().take(limit).collect();
    }
    let mut starts: Vec<&'static City> = Vec::new();
    let mut contains: Vec<&'static City> = Vec::new();
    for c in CITIES {
        let name_l = c.name.to_ascii_lowercase();
        let country_l = c.country.to_ascii_lowercase();
        let tz_l = c.tz.to_ascii_lowercase();
        if name_l.starts_with(&q) || country_l.starts_with(&q) {
            starts.push(c);
        } else if name_l.contains(&q) || country_l.contains(&q) || tz_l.contains(&q) {
            contains.push(c);
        }
    }
    starts.extend(contains);
    starts.truncate(limit);
    starts
}
