-- Add description and role columns to phases table.
ALTER TABLE phases ADD COLUMN description TEXT;
ALTER TABLE phases ADD COLUMN role TEXT;
