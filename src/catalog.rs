use crate::{
    browser::{BrowserView, Facet, NavigatorView},
    model::*,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, params, params_from_iter, types::Value};
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
}

#[derive(Clone, Debug, Default)]
pub struct LibraryView {
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
        Ok(Self {
            conn,
            facets: RefCell::new(None),
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
                }
            }
        }
        tx.commit()?;
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
        }
        tx.commit()?;
        *self.facets.borrow_mut() = None;
        Ok(())
    }
    // Only called after every required server page succeeded. Partial syncs never delete.
    pub fn finish_sync(&mut self, generation: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM tracks WHERE generation != ?1", [generation])?;
        tx.execute("DELETE FROM albums WHERE generation != ?1", [generation])?;
        tx.commit()?;
        *self.facets.borrow_mut() = None;
        Ok(())
    }
    pub fn count(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))?)
    }
    fn filters(query: &LibraryQuery) -> (String, Vec<Value>) {
        let mut sql = " WHERE 1=1".to_string();
        let mut values = Vec::new();
        let text = normalize(query.text.trim());
        if !text.is_empty() {
            if text.chars().count() >= 3 {
                sql.push_str(
                    " AND rowid IN (SELECT rowid FROM track_search WHERE track_search MATCH ?)",
                );
                values.push(Value::Text(format!("\"{}\"", text.replace('"', "\"\""))));
            } else {
                sql.push_str(" AND instr(search, ?) > 0");
                values.push(Value::Text(text));
            }
        }
        if !query.artist.is_empty() {
            sql.push_str(" AND (artist_matches(artist,?) OR artist_matches(album_artist,?))");
            values.extend([
                Value::Text(query.artist.clone()),
                Value::Text(query.artist.clone()),
            ]);
        }
        if !query.performer.is_empty() {
            sql.push_str(" AND (artist_matches(artist,?) OR EXISTS (SELECT 1 FROM json_each(tracks.data,'$.artists') a WHERE artist_matches(json_extract(a.value,'$.name'),?)))");
            values.extend([
                Value::Text(query.performer.clone()),
                Value::Text(query.performer.clone()),
            ]);
        }
        if !query.genre.is_empty() {
            sql.push_str(" AND instr(' · ' || genre || ' · ',' · ' || ? || ' · ')>0");
            values.push(Value::Text(query.genre.clone()));
        }
        if !query.selection.genres.is_empty() {
            sql.push_str(" AND (");
            for (index, genre) in query.selection.genres.iter().enumerate() {
                if index > 0 {
                    sql.push_str(" OR ");
                }
                sql.push_str("instr(' · ' || genre || ' · ',' · ' || ? || ' · ')>0");
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
                sql.push_str("(artist_matches(album_artist,?) OR EXISTS (SELECT 1 FROM json_each(tracks.data,'$.albumArtists') a WHERE artist_matches(json_extract(a.value,'$.name'),?)))");
                values.push(Value::Text(artist.clone()));
                values.push(Value::Text(artist.clone()));
            }
            sql.push(')');
        }
        if !query.selection.albums.is_empty() {
            sql.push_str(" AND album_id IN (");
            sql.push_str(&vec!["?"; query.selection.albums.len()].join(","));
            sql.push(')');
            values.extend(query.selection.albums.iter().cloned().map(Value::Text));
        }
        if let Some(year) = query.year {
            sql.push_str(" AND year=?");
            values.push(Value::Integer(year as i64));
        }
        if query.favorites {
            sql.push_str(" AND starred=1");
        }
        (sql, values)
    }
    fn order(query: &LibraryQuery) -> String {
        let column = match query.sort {
            Sort::Album => "album COLLATE NOCASE",
            Sort::Artist => "artist COLLATE NOCASE",
            Sort::Title => "title COLLATE NOCASE",
            Sort::Year => "year",
            Sort::Added => "rowid",
            Sort::Duration => "duration",
            Sort::Rating => "rating",
            Sort::Plays => "plays",
            Sort::Format => "format",
        };
        format!(
            " ORDER BY {column} {}, album_id,disc,track,id",
            if query.descending { "DESC" } else { "ASC" }
        )
    }
    pub fn tracks(
        &self,
        query: &LibraryQuery,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Arc<Track>>> {
        let (filter, mut values) = Self::filters(query);
        let sql = format!(
            "SELECT data FROM tracks{filter}{} LIMIT ? OFFSET ?",
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
            "SELECT id FROM tracks{filter}{}",
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
        let album_sort = match query.sort {
            Sort::Artist => "artist COLLATE NOCASE",
            Sort::Year => "year",
            _ => "name COLLATE NOCASE",
        };
        let mut stmt = self.conn.prepare(&format!("SELECT data FROM albums WHERE id IN (SELECT DISTINCT album_id FROM tracks{filter}) ORDER BY {album_sort} {},name COLLATE NOCASE,id",if query.descending { "DESC" } else { "ASC" }))?;
        let albums = stmt
            .query_map(params_from_iter(values), |r| r.get::<_, String>(0))?
            .map(|r| Ok(Arc::new(serde_json::from_str(&r?)?)))
            .collect::<Result<Vec<_>>>()?;
        let facets = self.facets()?;
        Ok(LibraryView {
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
        query.selection.albums = vec![id.into()];
        query.sort = Sort::Album;
        query.descending = false;
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
