-- The original default bound receiving to loopback. mDNS could still announce
-- the device on the LAN, leaving peers stuck connecting to an unreachable
-- service. Migrate only the exact old default so explicit custom addresses are
-- preserved.
UPDATE settings
SET listen_address = '0.0.0.0:0'
WHERE listen_address = '127.0.0.1:0';
