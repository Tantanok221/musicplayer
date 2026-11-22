-- Frozen catalog schema from the v1 build. Never regenerate this fixture in place.
-- Includes FTS5, triggers, indexes and sorting cache tables; FTS rows are built by the insert triggers.
PRAGMA foreign_keys=ON;
PRAGMA user_version=1;
CREATE TABLE albums (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, artist TEXT NOT NULL,
                year INTEGER, genre TEXT NOT NULL, generation INTEGER NOT NULL, data TEXT NOT NULL);
CREATE TABLE artist_profiles (key TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE listen_submissions (id TEXT PRIMARY KEY, track_id TEXT NOT NULL, started_ms INTEGER NOT NULL, state TEXT NOT NULL);
CREATE TABLE playlists (id TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE sort_albums (
            id TEXT PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
            title BLOB NOT NULL,artist BLOB NOT NULL,artist_article BLOB NOT NULL,
            album_artist BLOB NOT NULL,album_artist_article BLOB NOT NULL,album BLOB NOT NULL,
            original_year INTEGER,original_month INTEGER,original_day INTEGER,
            release_year INTEGER,release_month INTEGER,release_day INTEGER,
            added INTEGER,rating INTEGER,plays INTEGER,duration REAL,format BLOB NOT NULL,release_type INTEGER NOT NULL);
CREATE TABLE sort_tracks (
            id TEXT PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
            title BLOB NOT NULL,artist BLOB NOT NULL,artist_article BLOB NOT NULL,
            album_artist BLOB NOT NULL,album_artist_article BLOB NOT NULL,album BLOB NOT NULL,
            original_year INTEGER,original_month INTEGER,original_day INTEGER,
            release_year INTEGER,release_month INTEGER,release_day INTEGER,
            added INTEGER,rating INTEGER,plays INTEGER,duration REAL,format BLOB NOT NULL,release_type INTEGER NOT NULL);
CREATE TABLE state (key TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE VIRTUAL TABLE track_search USING fts5(search, content='tracks', content_rowid='rowid', tokenize='trigram');
CREATE TABLE tracks (
                rowid INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, album_id TEXT NOT NULL,
                title TEXT NOT NULL, artist TEXT NOT NULL, album_artist TEXT NOT NULL,
                album TEXT NOT NULL, genre TEXT NOT NULL, year INTEGER, disc INTEGER, track INTEGER,
                duration REAL NOT NULL, format TEXT NOT NULL, starred INTEGER NOT NULL,
                rating INTEGER NOT NULL, plays INTEGER NOT NULL, search TEXT NOT NULL,
                generation INTEGER NOT NULL, data TEXT NOT NULL);
CREATE INDEX albums_artist ON albums(artist);
CREATE INDEX albums_name ON albums(name);
CREATE INDEX sort_albums_added ON sort_albums(added,id);
CREATE INDEX sort_albums_artist_album ON sort_albums(album_artist_article,album,id);
CREATE INDEX sort_albums_chronology ON sort_albums(album_artist_article,original_year,original_month,original_day,title,id);
CREATE INDEX sort_albums_title ON sort_albums(title,id);
CREATE INDEX sort_tracks_added ON sort_tracks(added,id);
CREATE INDEX sort_tracks_artist_album ON sort_tracks(album_artist_article,album,id);
CREATE INDEX sort_tracks_chronology ON sort_tracks(album_artist_article,original_year,original_month,original_day,title,id);
CREATE INDEX sort_tracks_title ON sort_tracks(title,id);
CREATE INDEX tracks_album ON tracks(album_id,disc,track);
CREATE INDEX tracks_album_sort ON tracks(album COLLATE NOCASE,album_id,disc,track,id);
CREATE INDEX tracks_artist ON tracks(artist);
CREATE INDEX tracks_artist_sort ON tracks(artist COLLATE NOCASE,album_id,disc,track,id);
CREATE INDEX tracks_cover ON tracks(album_id,json_extract(data,'$.coverArt'));
CREATE INDEX tracks_starred ON tracks(starred);
CREATE INDEX tracks_title_sort ON tracks(title COLLATE NOCASE,album_id,disc,track,id);
CREATE INDEX tracks_year ON tracks(year);
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
                INSERT INTO track_search(track_search,rowid,search) VALUES('delete',old.rowid,old.search); END;
CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
                INSERT INTO track_search(rowid,search) VALUES (new.rowid,new.search); END;
CREATE TRIGGER tracks_au AFTER UPDATE ON tracks BEGIN
                INSERT INTO track_search(track_search,rowid,search) VALUES('delete',old.rowid,old.search);
                INSERT INTO track_search(rowid,search) VALUES(new.rowid,new.search); END;
INSERT INTO tracks VALUES(NULL,'song-1','album-1','Frozen Moon','Fixture Artist','Fixture Artist','Winter Lights','Ambient','2024','1','1','180.0','flac','0','5','7','frozen moon fixture artist winter lights ambient','1','{"id": "song-1", "title": "Frozen Moon", "albumId": "album-1", "album": "Winter Lights", "artist": "Fixture Artist", "artistId": "artist-1", "albumArtist": "Fixture Artist", "genre": "Ambient", "year": 2024, "track": 1, "discNumber": 1, "duration": 180.0, "suffix": "flac", "userRating": 5, "playCount": 7}');
INSERT INTO tracks VALUES(NULL,'song-2','album-1','Night River','Fixture Artist','Fixture Artist','Winter Lights','Ambient','2024','1','2','180.0','flac','0','0','0','night river fixture artist winter lights ambient','1','{"id": "song-2", "title": "Night River", "albumId": "album-1", "album": "Winter Lights", "artist": "Fixture Artist", "artistId": "artist-1", "albumArtist": "Fixture Artist", "genre": "Ambient", "year": 2024, "track": 2, "discNumber": 1, "duration": 180.0, "suffix": "flac", "userRating": null, "playCount": null}');
INSERT INTO albums VALUES('album-1','Winter Lights','Fixture Artist',2024,'Ambient',1,'{"id": "album-1", "name": "Winter Lights", "artist": "Fixture Artist", "year": 2024, "genre": "Ambient", "songCount": 2, "duration": 360.0}');
INSERT INTO state VALUES('queue','{"ids": ["song-1", "song-2"], "index": 1, "position": 42.5, "listen": null}');
INSERT INTO playlists VALUES('playlist-1','{"id": "playlist-1", "name": "Winter favorites", "songCount": 1, "duration": 180.0, "entry": [{"id": "song-2", "title": "Night River", "albumId": "album-1", "album": "Winter Lights", "artist": "Fixture Artist", "artistId": "artist-1", "albumArtist": "Fixture Artist", "genre": "Ambient", "year": 2024, "track": 2, "discNumber": 1, "duration": 180.0, "suffix": "flac", "userRating": null, "playCount": null}]}');
INSERT INTO artist_profiles VALUES('artist-1','{"id": "artist-1", "name": "Fixture Artist", "biography": "A frozen artist biography.", "popular": [{"id": "song-1", "title": "Frozen Moon", "albumId": "album-1", "album": "Winter Lights", "artist": "Fixture Artist", "artistId": "artist-1", "albumArtist": "Fixture Artist", "genre": "Ambient", "year": 2024, "track": 1, "discNumber": 1, "duration": 180.0, "suffix": "flac", "userRating": 5, "playCount": 7}], "complete": true, "fetched_ms": 1700000000000}');
INSERT INTO listen_submissions VALUES('101','song-1',1700000000000,'pending');
INSERT INTO listen_submissions VALUES('102','song-2',1700000001000,'confirmed');
