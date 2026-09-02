-- Forgetting a device was a permanent, one-way decision: a revoked row was
-- hidden from discovery and skipped by automatic pairing, with no way back in
-- the UI. A device is simply reachable or it is not, so the concept is gone
-- and every past revocation is lifted. The column is retained so old rows
-- still read, but nothing writes it any more.
UPDATE trusted_peers SET revoked_at = NULL WHERE revoked_at IS NOT NULL;
