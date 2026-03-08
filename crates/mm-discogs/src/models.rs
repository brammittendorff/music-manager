use serde::{Deserialize, Serialize};

// ─── Search response ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DiscogsSearchResponse {
    pub pagination: Pagination,
    pub results: Vec<DiscogsRelease>,
}

#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub page: u32,
    pub pages: u32,
    pub per_page: u32,
    pub items: u32,
}

// ─── Release (from search results) ───────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscogsRelease {
    pub id: u32,
    pub title: String,
    pub country: Option<String>,
    pub year: Option<String>,
    pub genre: Option<Vec<String>>,
    pub style: Option<Vec<String>>,
    pub format: Option<Vec<String>>,
    pub label: Option<Vec<String>>,
    pub catno: Option<String>,
    pub thumb: Option<String>,
    pub uri: Option<String>,
    pub master_url: Option<String>,
    pub resource_url: Option<String>,
    pub community: Option<Community>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Community {
    pub have: u32,
    pub want: u32,
}

// ─── Marketplace stats ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketplaceStats {
    pub lowest_price: Option<MarketplacePrice>,
    pub num_for_sale: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketplacePrice {
    pub value: f64,
    pub currency: String,
}

impl DiscogsRelease {
    /// Split "Artist – Title" Discogs format into artist and title parts.
    pub fn split_artist_title(&self) -> (String, String) {
        // Try en dash first (Discogs standard), then ASCII hyphen fallback
        let sep = self.title.find(" \u{2013} ").map(|i| (i, " \u{2013} ".len()))
            .or_else(|| self.title.find(" - ").map(|i| (i, 3)));
        if let Some((pos, len)) = sep {
            let artist = self.title[..pos].trim().to_owned();
            let title = self.title[pos + len..].trim().to_owned();
            (artist, title)
        } else {
            (String::new(), self.title.clone())
        }
    }

    pub fn year_as_i32(&self) -> Option<i32> {
        self.year
            .as_deref()
            .and_then(|y| y.parse::<i32>().ok())
    }

    pub fn primary_label(&self) -> Option<String> {
        self.label.as_ref()?.first().cloned()
    }

    pub fn formats_vec(&self) -> Vec<String> {
        self.format.clone().unwrap_or_default()
    }

    pub fn genres_vec(&self) -> Vec<String> {
        self.genre.clone().unwrap_or_default()
    }

    pub fn styles_vec(&self) -> Vec<String> {
        self.style.clone().unwrap_or_default()
    }

    pub fn discogs_url(&self) -> String {
        self.uri
            .clone()
            .unwrap_or_else(|| format!("https://www.discogs.com/release/{}", self.id))
    }
}
