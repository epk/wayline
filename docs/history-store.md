# Latency history

The resident monitor retains 64 samples per series in memory.
The control helper aggregates complete minutes every five minutes.

Persistent history uses the router's SQLite command.
No database runtime links into the resident monitor.
The database uses 4 KiB pages, an 8 MiB page limit, a 128 KiB cache, and no mmap.
Transactions use FULL synchronization and a truncated rollback journal.

Each row contains minute, series identity, minimum, median, maximum, sent count, and loss count.
A composite primary key prevents duplicate minute rows.
An update replaces a row only when it contains more attempts.
Collection prunes rows outside the previous 24 hours.
It also limits retained series to the latest 45 identities.

Persistent mode stores data under `/etc/wayline/history`.
Memory mode stores data under `/var/run/wayline/history`.
Both directories require root ownership and mode 0700.
Changing modes does not delete the other store.
A flash error appears as a failed control request or scheduler operation.

The UI overlays recent memory samples onto stored bins.
One-minute views use minute aggregates.
Five-minute views use the median of minute medians.
This value is not the exact median of all raw attempts.
Bands preserve minimum and maximum values.
Missing bins produce gaps.

A crash can lose the latest five minutes before collection.
A restart within a minute can omit partial attempts from that minute.
An unsynchronized clock prevents persistent collection.
A backward clock change prunes future bins.
The store does not promise forensic completeness.

Settings and profile identity form part of the series identity through discovered paths and targets.
A changed interface, source, mark, or profile starts a separate history series.
