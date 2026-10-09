-- Paused recordings retain their audio and never enter the processing queue.
ALTER TABLE meetings ADD COLUMN paused INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1));
ALTER TABLE meetings ADD COLUMN needs_full_transcription INTEGER NOT NULL DEFAULT 0 CHECK (needs_full_transcription IN (0, 1));
