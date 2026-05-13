-- Migration 20260513000001: add name, description, and tags to segments
ALTER TABLE segments
    ADD COLUMN name        TEXT NOT NULL DEFAULT '',
    ADD COLUMN description TEXT NOT NULL DEFAULT '',
    ADD COLUMN tags        TEXT[] NOT NULL DEFAULT '{}';
