-- The SAS is a six-digit value derived only after mutual transcript proofs
-- verify. Keeping just the normalized display value allows restart-safe user
-- comparison without persisting proofs, transcripts, certificates, or secrets.
ALTER TABLE pending_pairings ADD COLUMN sas_code TEXT CHECK (
  sas_code IS NULL OR sas_code GLOB '[0-9][0-9][0-9] [0-9][0-9][0-9]'
);
