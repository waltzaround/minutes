-- Keep what the transcript said alongside resolved values: the speaker
-- label the model named as owner, and the due date as spoken ("Friday").
ALTER TABLE action_items ADD COLUMN owner_label TEXT;
ALTER TABLE action_items ADD COLUMN due_text TEXT;
