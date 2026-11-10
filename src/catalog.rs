use crate::{
    artists::{ArtistPageData, ArtistProfile},
    browser::{BrowserView, Facet, NavigatorView},
    listening::ListenReport,
    model::*,
    sorting::{self, Scope, SortSpec},
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
};

#[derive(Default)]
struct NavigatorIndex(BTreeMap<String, (Facet, HashSet<String>)>);
impl NavigatorIndex {
    fn add(&mut self, key: String, label: &str, detail: &str, track: &Track) {
        if label.trim().is_empty() {
            return;
        }
        let (facet, albums) = self.0.entry(key).or_insert_with(|| {
            (
                Facet {
                    id: label.into(),
                    label: label.into(),
                    ..Default::default()
                },
                HashSet::new(),
            )
        });
        if facet.artwork.is_empty() {
            facet.artwork = track.cover_art.clone();
        }
        if facet.detail.is_empty() {
            facet.detail = detail.into();
        }
        albums.insert(if track.album_id.is_empty() {
            format!("track:{}", track.id)
        } else {
            format!("album:{}", track.album_id)
        });
    }
    fn finish(self) -> Vec<Facet> {
        self.0
            .into_values()
            .map(|(mut facet, albums)| {
                facet.count = albums.len();
                facet
            })
            .collect()
    }
}

#[derive(Clone)]
struct Facets {
    navigator: NavigatorView,
    artists: Vec<String>,
    genres: Vec<String>,
    years: Vec<i32>,
    album_count: usize,
}
pub struct Catalog {
    conn: Connection,
    facets: RefCell<Option<Facets>>,
    revision: u64,
}

#[derive(Clone, Debug, Default)]
pub struct LibraryView {
    pub revision: u64,
    pub sort_fields: Vec<sorting::Field>,
    pub albums: Vec<Arc<Album>>,
    pub browser: BrowserView,
    pub navigator: NavigatorView,
    pub tracks: Vec<Arc<Track>>,
    pub track_count: usize,
    pub album_count: usize,
    pub artists: Vec<String>,
    pub genres: Vec<String>,
    pub years: Vec<i32>,
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(conn: Connection) -> Result<Self> {
        conn.create_scalar_function(
            "label_matches",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let label = context.get::<Option<String>>(0)?.unwrap_or_default();
                let target = context.get::<String>(1)?;
                Ok(!label.trim().is_empty() && normalize(label.trim()) == normalize(target.trim()))
            },
        )?;
        conn.create_scalar_function(
            "artist_matches",
            2,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let label = context.get::<String>(0)?;
                let name = context.get::<String>(1)?;
                Ok(normalize(&label) == normalize(&name)
                    || split_artist_names(&label)
                        .iter()
                        .any(|part| normalize(part) == normalize(&name)))
            },
        )?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS tracks (
                rowid INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, album_id TEXT NOT NULL,
                title TEXT NOT NULL, artist TEXT NOT NULL, album_artist TEXT NOT NULL,
                album TEXT NOT NULL, genre TEXT NOT NULL, year INTEGER, disc INTEGER, track INTEGER,
                duration REAL NOT NULL, format TEXT NOT NULL, starred INTEGER NOT NULL,
                rating INTEGER NOT NULL, plays INTEGER NOT NULL, search TEXT NOT NULL,
                generation INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album_id,disc,track);
            CREATE INDEX IF NOT EXISTS tracks_album_sort ON tracks(album COLLATE NOCASE,album_id,disc,track,id);
            CREATE INDEX IF NOT EXISTS tracks_title_sort ON tracks(title COLLATE NOCASE,album_id,disc,track,id);
            CREATE INDEX IF NOT EXISTS tracks_artist_sort ON tracks(artist COLLATE NOCASE,album_id,disc,track,id);
            CREATE INDEX IF NOT EXISTS tracks_artist ON tracks(artist);
            CREATE INDEX IF NOT EXISTS tracks_year ON tracks(year);
            CREATE INDEX IF NOT EXISTS tracks_starred ON tracks(starred);
            CREATE TABLE IF NOT EXISTS albums (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, artist TEXT NOT NULL,
                year INTEGER, genre TEXT NOT NULL, generation INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS albums_name ON albums(name);
            CREATE INDEX IF NOT EXISTS albums_artist ON albums(artist);
            CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS artist_profiles (key TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS listen_submissions (id TEXT PRIMARY KEY, track_id TEXT NOT NULL, started_ms INTEGER NOT NULL, state TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS playlists (id TEXT PRIMARY KEY, data TEXT NOT NULL);
            CREATE VIRTUAL TABLE IF NOT EXISTS track_search USING fts5(search, content='tracks', content_rowid='rowid', tokenize='trigram');
            CREATE TRIGGER IF NOT EXISTS tracks_ai AFTER INSERT ON tracks BEGIN
                INSERT INTO track_search(rowid,search) VALUES (new.rowid,new.search); END;
            CREATE TRIGGER IF NOT EXISTS tracks_ad AFTER DELETE ON tracks BEGIN
                INSERT INTO track_search(track_search,rowid,search) VALUES('delete',old.rowid,old.search); END;
            CREATE TRIGGER IF NOT EXISTS tracks_au AFTER UPDATE ON tracks BEGIN
                INSERT INTO track_search(track_search,rowid,search) VALUES('delete',old.rowid,old.search);
                INSERT INTO track_search(rowid,search) VALUES(new.rowid,new.search); END;
            PRAGMA user_version=1;")?;
        sorting::initialize(&conn)?;
        Ok(Self {
            conn,
            facets: RefCell::new(None),
            revision: 0,
        })
    }
    pub fn put_tracks(&mut self, tracks: &[Track], generation: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("INSERT INTO tracks
                (id,album_id,title,artist,album_artist,album,genre,year,disc,track,duration,format,starred,rating,plays,search,generation,data)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
                ON CONFLICT(id) DO UPDATE SET album_id=excluded.album_id,title=excluded.title,artist=excluded.artist,
                album_artist=excluded.album_artist,album=excluded.album,genre=excluded.genre,year=excluded.year,
                disc=excluded.disc,track=excluded.track,duration=excluded.duration,format=excluded.format,
                starred=excluded.starred,rating=excluded.rating,plays=excluded.plays,search=excluded.search,generation=excluded.generation,data=excluded.data")?;
            let mut albums = tx.prepare_cached("INSERT INTO albums(id,name,artist,year,genre,generation,data) VALUES(?1,?2,?3,?4,?5,?6,?7)
                ON CONFLICT(id) DO UPDATE SET generation=excluded.generation")?;
            let mut sort_album_ids = HashSet::new();
            for track in tracks {
                if track.id.is_empty() {
                    anyhow::bail!("Server returned a track without an ID");
                }
                let genre = track.genres_label();
                let credits = track
                    .contributors
                    .iter()
                    .map(|c| c.artist.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let search = normalize(&format!(
                    "{} {} {} {} {} {}",
                    track.title,
                    track.artist_label(),
                    track.album,
                    track.album_artist_label(),
                    genre,
                    credits
                ));
                stmt.execute(params![
                    track.id,
                    track.album_id,
                    track.title,
                    track.artist_label(),
                    track.album_artist_label(),
                    track.album,
                    genre,
                    track.year,
                    track.disc_number,
                    track.track,
                    track.duration,
                    track.suffix,
                    track.starred.is_some(),
                    track.user_rating.unwrap_or(0),
                    track.play_count.unwrap_or(0),
                    search,
                    generation,
                    serde_json::to_string(track)?
                ])?;
                sorting::put_track(&tx, track)?;
                if !track.album_id.is_empty() {
                    let album = Album {
                        id: track.album_id.clone(),
                        name: track.album.clone(),
                        artist: track.album_artist_label().into(),
                        artist_id: track.artist_id.clone(),
                        cover_art: track.cover_art.clone(),
                        year: track.year,
                        genre: genre.clone(),
                        ..Default::default()
                    };
                    albums.execute(params![
                        album.id,
                        album.name,
                        album.artist,
                        album.year,
                        album.genre,
                        generation,
                        serde_json::to_string(&album)?
                    ])?;
                    // Preserve authoritative album values when a track creates a placeholder.
                    if sort_album_ids.insert(album.id.clone()) {
                        let data: String =
                            tx.query_row("SELECT data FROM albums WHERE id=?", [&album.id], |r| {
                                r.get(0)
                            })?;
                        sorting::put_album(&tx, &serde_json::from_str(&data)?)?;
                    }
                }
            }
        }
        tx.commit()?;
        self.revision += 1;
        *self.facets.borrow_mut() = None;
        Ok(())
    }
    pub fn put_albums(&mut self, albums: &[Album], generation: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for a in albums {
            if a.id.is_empty() {
                anyhow::bail!("Server returned an album without an ID");
            }
            tx.execute("INSERT INTO albums(id,name,artist,year,genre,generation,data) VALUES(?1,?2,?3,?4,?5,?6,?7)
                ON CONFLICT(id) DO UPDATE SET name=excluded.name,artist=excluded.artist,year=excluded.year,genre=excluded.genre,generation=excluded.generation,data=excluded.data",
                params![a.id,a.name,a.artist,a.year,a.genre,generation,serde_json::to_string(a)?])?;
            sorting::put_album(&tx, a)?;
        }
        tx.commit()?;
        self.revision += 1;
        *self.facets.borrow_mut() = None;
        Ok(())
    }
    pub fn album(&self, id: &str) -> Result<Option<Arc<Album>>> {
        self.conn
            .query_row("SELECT data FROM albums WHERE id=?", [id], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .map(|data| Ok(Arc::new(serde_json::from_str(&data)?)))
            .transpose()
    }
    /// Patch annotations without changing sync ownership, release metadata, or songs.
    pub fn update_album_feedback(
        &mut self,
        id: &str,
        favorite: Option<bool>,
        rating: Option<u8>,
    ) -> Result<Option<Arc<Album>>> {
        let Some(old) = self.album(id)? else {
            return Ok(None);
        };
        let mut album = (*old).clone();
        if let Some(favorite) = favorite {
            album.starred = favorite.then(|| chrono::Utc::now().to_rfc3339());
        }
        if let Some(rating) = rating {
            album.user_rating = Some(rating.min(5));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE albums SET data=?1 WHERE id=?2",
            params![serde_json::to_string(&album)?, id],
        )?;
        sorting::put_album(&tx, &album)?;
        tx.commit()?;
        self.revision += 1;
        Ok(Some(Arc::new(album)))
    }
    // Only called after every required server page succeeded. Partial syncs never delete.
    pub fn finish_sync(&mut self, generation: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM tracks WHERE generation != ?1", [generation])?;
        tx.execute("DELETE FROM albums WHERE generation != ?1", [generation])?;
        tx.commit()?;
        self.revision += 1;
        *self.facets.borrow_mut() = None;
        Ok(())
    }
    pub fn count(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))?)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn filters(query: &LibraryQuery) -> (String, Vec<Value>) {
        let mut sql = " WHERE 1=1".to_string();
        let mut values = Vec::new();
        let text = normalize(query.text.trim());
        if !text.is_empty() {
            if text.chars().count() >= 3 {
                sql.push_str(
                    " AND tracks.rowid IN (SELECT rowid FROM track_search WHERE track_search MATCH ?)",
                );
                values.push(Value::Text(format!("\"{}\"", text.replace('"', "\"\""))));
            } else {
                sql.push_str(" AND instr(tracks.search, ?) > 0");
                values.push(Value::Text(text));
            }
        }
        if !query.artist.is_empty() {
            sql.push_str(
                " AND (artist_matches(tracks.artist,?) OR artist_matches(tracks.album_artist,?))",
            );
            values.extend([
                Value::Text(query.artist.clone()),
                Value::Text(query.artist.clone()),
            ]);
        }
        if !query.performer.is_empty() {
            sql.push_str(" AND (artist_matches(tracks.artist,?) OR EXISTS (SELECT 1 FROM json_each(tracks.data,'$.artists') a WHERE artist_matches(json_extract(a.value,'$.name'),?)))");
            values.extend([
                Value::Text(query.performer.clone()),
                Value::Text(query.performer.clone()),
            ]);
        }
        if !query.genre.is_empty() {
            sql.push_str(" AND instr(' · ' || tracks.genre || ' · ',' · ' || ? || ' · ')>0");
            values.push(Value::Text(query.genre.clone()));
        }
        if !query.selection.genres.is_empty() {
            sql.push_str(" AND (");
            for (index, genre) in query.selection.genres.iter().enumerate() {
                if index > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("instr(' · ' || tracks.genre || ' · ',' · ' || ? || ' · ')>0");
                values.push(Value::Text(genre.clone()));
            }
            sql.push(')');
        }
        if !query.selection.artists.is_empty() {
            sql.push_str(" AND (");
            for (index, artist) in query.selection.artists.iter().enumerate() {
                if index > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("(artist_matches(tracks.album_artist,?) OR EXISTS (SELECT 1 FROM json_each(tracks.data,'$.albumArtists') a WHERE artist_matches(json_extract(a.value,'$.name'),?)))");
                values.push(Value::Text(artist.clone()));
                values.push(Value::Text(artist.clone()));
            }
            sql.push(')');
        }
        if !query.selection.albums.is_empty() {
            sql.push_str(" AND tracks.album_id IN (");
            sql.push_str(&vec!["?"; query.selection.albums.len()].join(","));
            sql.push(')');
            values.extend(query.selection.albums.iter().cloned().map(Value::Text));
        }
        if let Some(year) = query.year {
            sql.push_str(" AND tracks.year=?");
            values.push(Value::Integer(year as i64));
        }
        if !query.record_label.trim().is_empty() {
            sql.push_str(" AND tracks.album_id IN (SELECT label_album.id FROM albums label_album WHERE EXISTS (SELECT 1 FROM json_each(label_album.data,'$.recordLabels') record_label WHERE label_matches(json_extract(record_label.value,'$.name'),?)))");
            values.push(Value::Text(query.record_label.trim().into()));
        }
        if query.favorites {
            if query
                .ordering
                .as_ref()
                .is_some_and(|sort| sort.scope == Scope::Albums)
            {
                sql.push_str(" AND (tracks.starred=1 OR tracks.album_id IN (SELECT id FROM albums WHERE json_extract(data,'$.starred') IS NOT NULL))");
            } else {
                sql.push_str(" AND tracks.starred=1");
            }
        }
        (sql, values)
    }
    fn order(query: &LibraryQuery) -> String {
        let spec = query
            .ordering
            .clone()
            .unwrap_or_else(|| SortSpec::legacy(query.sort, query.descending, Scope::Tracks));
        sorting::order(&spec, true)
    }
    pub fn tracks(
        &self,
        query: &LibraryQuery,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Arc<Track>>> {
        let (filter, mut values) = Self::filters(query);
        let sql = format!(
            "SELECT tracks.data FROM tracks{}{filter}{} LIMIT ? OFFSET ?",
            sorting::TRACK_JOINS,
            Self::order(query)
        );
        values.push(Value::Integer(limit as i64));
        values.push(Value::Integer(offset as i64));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values), |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(Arc::new(serde_json::from_str(&r?)?)))
            .collect()
    }
    pub fn matching_ids(&self, query: &LibraryQuery) -> Result<Vec<String>> {
        let (filter, values) = Self::filters(query);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT tracks.id FROM tracks{}{filter}{}",
            sorting::TRACK_JOINS,
            Self::order(query)
        ))?;
        Ok(stmt
            .query_map(params_from_iter(values), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn view(&self, query: &LibraryQuery) -> Result<LibraryView> {
        let (filter, values) = Self::filters(query);
        let track_count = self.conn.query_row(
            &format!("SELECT count(*) FROM tracks{filter}"),
            params_from_iter(values.clone()),
            |r| r.get(0),
        )?;
        let spec = query
            .ordering
            .clone()
            .unwrap_or_else(|| SortSpec::legacy(query.sort, query.descending, Scope::Albums))
            .validated(Scope::Albums);
        let mut stmt = self.conn.prepare(&format!("SELECT albums.data FROM albums{} WHERE albums.id IN (SELECT DISTINCT album_id FROM tracks{filter}){}", sorting::ALBUM_JOINS, sorting::order(&spec,false)))?;
        let albums = stmt
            .query_map(params_from_iter(values), |r| r.get::<_, String>(0))?
            .map(|r| Ok(Arc::new(serde_json::from_str(&r?)?)))
            .collect::<Result<Vec<_>>>()?;
        let facets = self.facets()?;
        Ok(LibraryView {
            revision: self.revision,
            sort_fields: sorting::available(
                &self.conn,
                query
                    .ordering
                    .as_ref()
                    .map_or(Scope::Albums, |spec| spec.scope),
            )?,
            albums,
            browser: self.browser_view(query)?,
            navigator: facets.navigator,
            tracks: self.tracks(query, 0, 500)?,
            track_count,
            album_count: facets.album_count,
            artists: facets.artists,
            genres: facets.genres,
            years: facets.years,
        })
    }
    pub fn browser_view(&self, query: &LibraryQuery) -> Result<BrowserView> {
        let mut upstream = query.clone();
        upstream.selection = Default::default();
        upstream.artist.clear();
        upstream.genre.clear();
        let (filter, values) = Self::filters(&upstream);
        let mut statement = self.conn.prepare(&format!(
            "SELECT genre,count(*) FROM tracks{filter} GROUP BY genre"
        ))?;
        let rows = statement.query_map(params_from_iter(values), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?))
        })?;
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for row in rows {
            let (genres, count) = row?;
            for genre in genres.split(" · ").filter(|s| !s.is_empty()) {
                *counts.entry(genre.into()).or_default() += count;
            }
        }
        let genres = counts
            .into_iter()
            .map(|(name, count)| Facet {
                id: name.clone(),
                label: name,
                count,
                ..Default::default()
            })
            .collect();
        upstream.selection.genres = query.selection.genres.clone();
        if !query.genre.is_empty() {
            upstream.genre = query.genre.clone();
        }
        let (filter, values) = Self::filters(&upstream);
        let mut statement = self.conn.prepare(&format!("SELECT min(data),album_id FROM tracks{filter} AND album_artist!='' GROUP BY album_artist,album_id"))?;
        let rows = statement.query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut artist_groups =
            std::collections::BTreeMap::<String, (Facet, std::collections::HashSet<String>)>::new();
        for row in rows {
            let (data, album_id) = row?;
            let track: Track = serde_json::from_str(&data)?;
            for credit in track.album_artist_credits() {
                let (facet, albums) =
                    artist_groups
                        .entry(normalize(&credit.name))
                        .or_insert_with(|| {
                            (
                                Facet {
                                    id: credit.name.clone(),
                                    label: credit.name.clone(),
                                    artwork: track.cover_art.clone(),
                                    detail: credit.id.clone(),
                                    ..Default::default()
                                },
                                std::collections::HashSet::new(),
                            )
                        });
                if facet.detail.is_empty() {
                    facet.detail = credit.id;
                }
                if facet.artwork.is_empty() {
                    facet.artwork = track.cover_art.clone();
                }
                albums.insert(album_id.clone());
            }
        }
        let artists = artist_groups
            .into_values()
            .map(|(mut facet, albums)| {
                facet.count = albums.len();
                facet
            })
            .collect();
        upstream.selection.artists = query.selection.artists.clone();
        upstream.artist = query.artist.clone();
        let (filter, values) = Self::filters(&upstream);
        let mut statement=self.conn.prepare(&format!("SELECT album_id,album,count(*),min(json_extract(data,'$.coverArt')),album_artist FROM tracks{filter} GROUP BY album_id ORDER BY album COLLATE NOCASE"))?;
        let albums = statement
            .query_map(params_from_iter(values), |r| {
                Ok(Facet {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    count: r.get(2)?,
                    artwork: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    detail: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(BrowserView {
            genres,
            artists,
            albums,
        })
    }
    pub fn album_tracks_matching(&self, query: &LibraryQuery, id: &str) -> Result<Vec<Arc<Track>>> {
        let mut query = query.clone();
        let scope = query
            .ordering
            .as_ref()
            .map_or(Scope::Tracks, |sort| sort.scope);
        query.selection.albums = vec![id.into()];
        query.sort = Sort::Album;
        query.descending = false;
        query.ordering = Some(SortSpec::legacy(Sort::Album, false, scope));
        self.tracks(&query, 0, i64::MAX as usize)
    }
    fn facets(&self) -> Result<Facets> {
        if let Some(facets) = self.facets.borrow().as_ref() {
            return Ok(facets.clone());
        }
        let mut artists = self.strings(
            "SELECT DISTINCT artist FROM albums WHERE artist!='' ORDER BY artist COLLATE NOCASE",
        )?.into_iter().flat_map(|artist| split_artist_names(&artist)).collect::<Vec<_>>();
        artists.sort_by_key(|artist| normalize(artist));
        artists.dedup_by(|left, right| normalize(left) == normalize(right));
        let genre_strings =
            self.strings("SELECT DISTINCT genre FROM tracks WHERE genre!='' ORDER BY genre")?;
        let mut genres = genre_strings
            .iter()
            .flat_map(|s| s.split(" · ").map(str::to_string))
            .collect::<Vec<_>>();
        genres.sort();
        genres.dedup();
        let mut years_stmt = self.conn.prepare(
            "SELECT DISTINCT year FROM tracks WHERE year IS NOT NULL ORDER BY year DESC",
        )?;
        let years = years_stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let album_count = self
            .conn
            .query_row("SELECT count(*) FROM albums", [], |r| r.get(0))?;
        let facets = Facets {
            navigator: self.navigator_view()?,
            artists,
            genres,
            years,
            album_count,
        };
        *self.facets.borrow_mut() = Some(facets.clone());
        Ok(facets)
    }
    fn navigator_view(&self) -> Result<NavigatorView> {
        let mut artists = NavigatorIndex::default();
        let mut album_artists = NavigatorIndex::default();
        let mut genres = NavigatorIndex::default();
        let mut years = NavigatorIndex::default();
        let mut statement = self.conn.prepare("SELECT data FROM tracks ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let track: Track = serde_json::from_str(&row?)?;
            for credit in track.artist_credits() {
                artists.add(normalize(&credit.name), &credit.name, &credit.id, &track);
            }
            for credit in track.album_artist_credits() {
                album_artists.add(normalize(&credit.name), &credit.name, &credit.id, &track);
            }
            for genre in track
                .genres_label()
                .split(" · ")
                .filter(|name| !name.is_empty())
            {
                genres.add(genre.into(), genre, "", &track);
            }
            if let Some(year) = track.year {
                let label = year.to_string();
                years.add(label.clone(), &label, "", &track);
            }
        }
        let mut years = years.finish();
        years.sort_by_key(|facet| std::cmp::Reverse(facet.id.parse::<i32>().unwrap_or_default()));
        Ok(NavigatorView {
            artists: artists.finish(),
            album_artists: album_artists.finish(),
            genres: genres.finish(),
            years,
        })
    }
    fn strings(&self, sql: &str) -> Result<Vec<String>> {
        Ok(self
            .conn
            .prepare(sql)?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn album_tracks(&self, id: &str) -> Result<Vec<Arc<Track>>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM tracks WHERE album_id=? ORDER BY disc,track,title")?;
        stmt.query_map([id], |r| r.get::<_, String>(0))?
            .map(|r| Ok(Arc::new(serde_json::from_str(&r?)?)))
            .collect()
    }
    pub fn artist_releases(&self, name: &str, id: &str) -> Result<Vec<Arc<Album>>> {
        let mut statement = self.conn.prepare(
            "SELECT a.data FROM albums a WHERE artist_matches(a.artist,?1)
             OR (?2!='' AND json_extract(a.data,'$.artistId')=?2)
             OR EXISTS (SELECT 1 FROM json_each(a.data,'$.artists') artist
                WHERE (?2!='' AND json_extract(artist.value,'$.id')=?2)
                   OR artist_matches(json_extract(artist.value,'$.name'),?1))
             OR EXISTS (SELECT 1 FROM tracks t WHERE t.album_id=a.id AND (
                artist_matches(t.album_artist,?1) OR EXISTS (SELECT 1 FROM json_each(t.data,'$.albumArtists') artist
                   WHERE (?2!='' AND json_extract(artist.value,'$.id')=?2)
                      OR artist_matches(json_extract(artist.value,'$.name'),?1))))
             ORDER BY a.year DESC,a.name COLLATE NOCASE,a.id",
        )?;
        statement
            .query_map(params![name, id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(Arc::new(serde_json::from_str(&row?)?)))
            .collect()
    }
    /// MusicBrainz release IDs of every album in the library.
    pub fn release_ids(&self) -> Result<std::collections::HashSet<String>> {
        let mut statement = self.conn.prepare(
            "SELECT json_extract(data,'$.musicBrainzId') FROM albums
             WHERE coalesce(json_extract(data,'$.musicBrainzId'),'')!=''",
        )?;
        Ok(statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn artist_tracks(&self, name: &str, id: &str) -> Result<Vec<Arc<Track>>> {
        let mut statement = self.conn.prepare("SELECT data FROM tracks WHERE artist_matches(artist,?1) OR artist_matches(album_artist,?1)
            OR EXISTS (SELECT 1 FROM json_each(tracks.data,'$.artists') a WHERE artist_matches(json_extract(a.value,'$.name'),?1) OR (?2!='' AND json_extract(a.value,'$.id')=?2))
            OR EXISTS (SELECT 1 FROM json_each(tracks.data,'$.albumArtists') a WHERE artist_matches(json_extract(a.value,'$.name'),?1) OR (?2!='' AND json_extract(a.value,'$.id')=?2))")?;
        statement
            .query_map(params![name, id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(Arc::new(serde_json::from_str(&row?)?)))
            .collect()
    }
    fn tracks_for_albums(&self, ids: &[String]) -> Result<Vec<Arc<Track>>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self.conn.prepare(
            "SELECT data FROM tracks WHERE album_id IN (SELECT value FROM json_each(?1)) ORDER BY disc,track,title,id",
        )?;
        statement
            .query_map([serde_json::to_string(ids)?], |row| row.get::<_, String>(0))?
            .map(|row| Ok(Arc::new(serde_json::from_str(&row?)?)))
            .collect()
    }
    pub fn artist_page(&self, name: &str, id: &str) -> Result<ArtistPageData> {
        let albums = self.artist_releases(name, id)?;
        let album_ids = albums
            .iter()
            .map(|album| album.id.clone())
            .collect::<Vec<_>>();
        let tracks = self.tracks_for_albums(&album_ids)?;
        let artist_tracks = self.artist_tracks(name, id)?;
        let mut most_played = artist_tracks.clone();
        most_played.retain(|track| track.play_count.unwrap_or(0) > 0);
        most_played.sort_by(|a, b| {
            b.play_count
                .unwrap_or(0)
                .cmp(&a.play_count.unwrap_or(0))
                .then_with(|| normalize(&a.title).cmp(&normalize(&b.title)))
                .then_with(|| a.id.cmp(&b.id))
        });
        most_played.truncate(10);
        let release_ids = albums
            .iter()
            .map(|album| album.id.as_str())
            .collect::<HashSet<_>>();
        let mut counts = BTreeMap::<String, (String, HashSet<String>)>::new();
        for (album_id, genres) in albums
            .iter()
            .map(|album| (album.id.as_str(), album.genre.clone()))
            .chain(
                artist_tracks
                    .iter()
                    .filter(|track| release_ids.contains(track.album_id.as_str()))
                    .map(|track| (track.album_id.as_str(), track.genres_label())),
            )
        {
            for genre in genres.split(" · ").filter(|genre| !genre.is_empty()) {
                counts
                    .entry(normalize(genre))
                    .or_insert_with(|| (genre.into(), HashSet::new()))
                    .1
                    .insert(album_id.into());
            }
        }
        let mut genres = counts
            .into_values()
            .map(|(name, albums)| (name, albums.len()))
            .collect::<Vec<_>>();
        genres.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(ArtistPageData {
            albums,
            tracks,
            most_played,
            genres: genres.into_iter().take(5).map(|item| item.0).collect(),
        })
    }
    pub fn artist_profile(&self, key: &str) -> Result<Option<ArtistProfile>> {
        let mut statement = self
            .conn
            .prepare("SELECT data FROM artist_profiles WHERE key=?")?;
        let mut rows = statement.query([key])?;
        rows.next()?
            .map(|row| Ok(serde_json::from_str(&row.get::<_, String>(0)?)?))
            .transpose()
    }
    /// Uses primary-key lookups and the cached navigator index; a full track scan
    /// per similar artist made the page wait over a second on large libraries.
    pub fn hydrate_artist_profile(&self, profile: &mut ArtistProfile) -> Result<()> {
        let mut seen = HashSet::new();
        let ids = profile
            .popular
            .iter()
            .filter(|track| seen.insert(track.id.clone()))
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let mut popular = self.by_ids(&ids)?;
        popular.truncate(10);
        profile.popular = popular;
        let mut names = HashSet::new();
        profile.similar.retain(|similar| {
            normalize(&similar.name) != normalize(&profile.name)
                && names.insert(normalize(&similar.name))
        });
        profile.similar.truncate(12);
        if profile.similar.is_empty() {
            return Ok(());
        }
        let navigator = self.facets()?.navigator;
        let credits = navigator.artists.iter().chain(&navigator.album_artists);
        let mut owned_names = HashSet::new();
        let mut owned_ids = HashSet::new();
        for facet in credits {
            owned_names.insert(normalize(&facet.label));
            if !facet.detail.is_empty() {
                owned_ids.insert(facet.detail.as_str());
            }
        }
        for similar in &mut profile.similar {
            similar.in_library = owned_names.contains(&normalize(&similar.name))
                || (!similar.id.is_empty() && owned_ids.contains(similar.id.as_str()));
        }
        Ok(())
    }
    pub fn invalidate_artist_profiles(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE artist_profiles SET data=json_set(data,'$.fetched_ms',0)",
            [],
        )?;
        self.conn.execute(
            "UPDATE state SET data=json_set(data,'$.fetched_ms',0) WHERE key LIKE 'top-songs:%' AND data!='null'",
            [],
        )?;
        Ok(())
    }
    pub fn save_artist_profile(&self, key: &str, profile: &ArtistProfile) -> Result<()> {
        self.conn.execute("INSERT INTO artist_profiles(key,data) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data", params![key, serde_json::to_string(profile)?])?;
        Ok(())
    }
    pub fn queue_scrobble(&self, report: &ListenReport) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO listen_submissions(id,track_id,started_ms,state) VALUES(?1,?2,?3,'pending')", params![report.id.to_string(), report.track_id, report.started_ms])?;
        Ok(())
    }
    pub fn pending_scrobbles(&self, account: &str) -> Result<Vec<ListenReport>> {
        let mut statement = self.conn.prepare("SELECT id,track_id,started_ms FROM listen_submissions WHERE state='pending' ORDER BY started_ms,id")?;
        Ok(statement
            .query_map([], |row| {
                Ok(ListenReport {
                    id: row.get::<_, String>(0)?.parse().unwrap_or_default(),
                    account: account.into(),
                    track_id: row.get(1)?,
                    started_ms: row.get(2)?,
                    submission: true,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn attempt_scrobble(&self, id: u64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE listen_submissions SET state='attempted' WHERE id=? AND state='pending'",
            [id.to_string()],
        )? == 1)
    }
    pub fn finish_scrobble(&self, id: u64, confirmed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE listen_submissions SET state=?1 WHERE id=?2",
            params![
                if confirmed { "confirmed" } else { "uncertain" },
                id.to_string()
            ],
        )?;
        Ok(())
    }
    pub fn refresh_play_count(&mut self, fresh: &Track) -> Result<Option<Arc<Track>>> {
        let Some(old) = self.by_ids(std::slice::from_ref(&fresh.id))?.pop() else {
            return Ok(None);
        };
        let mut track = (*old).clone();
        track.play_count = fresh.play_count;
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE tracks SET plays=?1,data=?2 WHERE id=?3",
            params![
                track.play_count.unwrap_or(0),
                serde_json::to_string(&track)?,
                track.id
            ],
        )?;
        sorting::put_track(&tx, &track)?;
        tx.commit()?;
        self.revision += 1;
        Ok(Some(Arc::new(track)))
    }
    pub fn album_queue_from(&self, track_id: &str) -> Result<Vec<Arc<Track>>> {
        let Some(track) = self.by_ids(&[track_id.into()])?.into_iter().next() else {
            anyhow::bail!("This track is no longer in the library. Refresh and try again.");
        };
        if track.album_id.is_empty() {
            return Ok(vec![track]);
        }
        let tracks = self.album_tracks(&track.album_id)?;
        let start = tracks
            .iter()
            .position(|candidate| candidate.id == track.id)
            .context("Track is missing from its album")?;
        Ok(tracks.into_iter().skip(start).collect())
    }
    pub fn by_ids(&self, ids: &[String]) -> Result<Vec<Arc<Track>>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT data FROM tracks WHERE id=?")?;
        let mut tracks = Vec::with_capacity(ids.len());
        for id in ids {
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                tracks.push(Arc::new(serde_json::from_str(&row.get::<_, String>(0)?)?));
            }
        }
        Ok(tracks)
    }
    pub fn set_state<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.execute(
            "INSERT INTO state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
    pub fn state<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        let mut stmt = self.conn.prepare("SELECT data FROM state WHERE key=?")?;
        let mut rows = stmt.query([key])?;
        if let Some(row) = rows.next()? {
            Ok(serde_json::from_str(&row.get::<_, String>(0)?)?)
        } else {
            Ok(T::default())
        }
    }
    pub fn save_playlists(&mut self, playlists: &[Playlist]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlists", [])?;
        for p in playlists {
            tx.execute(
                "INSERT INTO playlists VALUES(?1,?2)",
                params![p.id, serde_json::to_string(p)?],
            )?;
        }
        tx.commit()?;
        *self.facets.borrow_mut() = None;
        Ok(())
    }
    /// Refresh metadata while retaining previously downloaded offline songs.
    pub fn save_playlist_summaries(
        &mut self,
        playlists: &[Playlist],
        complete: bool,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for playlist in playlists {
            let mut metadata = serde_json::to_value(playlist)?;
            metadata.as_object_mut().unwrap().remove("entry");
            tx.execute("INSERT INTO playlists(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=json_patch(playlists.data,excluded.data)", params![playlist.id, metadata.to_string()])?;
        }
        if complete {
            let ids = playlists
                .iter()
                .map(|playlist| &playlist.id)
                .collect::<Vec<_>>();
            tx.execute(
                "DELETE FROM playlists WHERE id NOT IN (SELECT value FROM json_each(?1))",
                [serde_json::to_string(&ids)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Sidebar metadata never needs to deserialize every playlist's songs.
    pub fn playlist_summaries(&self) -> Result<Vec<Playlist>> {
        self.conn
            .prepare("SELECT json_remove(data, '$.entry') FROM playlists ORDER BY json_extract(data,'$.name') COLLATE NOCASE")?
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    pub fn playlist(&self, id: &str) -> Result<Option<Playlist>> {
        self.conn
            .query_row("SELECT data FROM playlists WHERE id=?", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|data| Ok(serde_json::from_str(&data)?))
            .transpose()
    }
    pub fn save_playlist(&self, playlist: &Playlist) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO playlists VALUES(?1,?2)",
            params![playlist.id, serde_json::to_string(playlist)?],
        )?;
        Ok(())
    }
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        self.conn
            .prepare(
                "SELECT data FROM playlists ORDER BY json_extract(data,'$.name') COLLATE NOCASE",
            )?
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
}
