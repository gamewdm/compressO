# Investigating a slow or cancelled video encode

Open **Settings → General → Troubleshooting → Show log files**. On Windows the
folder is `%LOCALAPPDATA%\com.compresso.app\logs\`. Detailed job logs are in its
`jobs` subfolder. Logs stay after cancellation, including when the partial output
is deleted. Times are UTC.

Under **Troubleshooting**, choose a performance log interval of **30 seconds,
60 seconds, 5 minutes, or 10 minutes** (default: 30 seconds). This applies to new
jobs; running jobs keep their starting interval. Choose how many job logs to keep
from **1 to 1,000** (default: **10**). These settings persist across app restarts.

Each input file is one job, even when many files are in one queue. Hardware
fallbacks and target-size retries go into that file’s existing log. Completed,
failed, cancelled, and interrupted jobs all count. Size-estimation samples and
files cancelled before they start do not create job logs. Image jobs have settings,
output, elapsed time, and a final status; periodic encoder metrics apply to
FFmpeg jobs.

Retention keeps the newest N finished job logs by start time, plus all active
jobs. Cleanup runs on completion, when the retention setting changes, and at
startup. Lowering the count removes older finished job logs immediately. General
application logs remain separate and retain their existing five-file policy.

Full video encodes write `[encode video=… batch=… pass=… attempt=… encoder=…]`
records. Estimation samples do not. Each attempt records:

- The command, process ID, logical CPU count, and FFmpeg's input/output description:
  duration, codec/profile, pixel format, resolution, frame rate, colour tags, stream
  mapping, and encoder settings where FFmpeg provides them.
- At the selected interval: elapsed time, latest frame count, fps, encoding speed, output
  bytes, media timestamp, duplicated/dropped frames, and age of the last progress
  report. A heartbeat continues even if FFmpeg stops reporting progress.
- On Windows: cumulative process CPU seconds, cumulative process read/write bytes,
  working set and peak working set, page fault count, system RAM load and available
  physical RAM. Missing counters are omitted; other platforms report that OS
  counters are unavailable.
- A completed, failed, cancelled, or interrupted record with the last progress and
  stderr tail. The first 80 stderr lines are logged immediately, the last 40 are
  retained for the final record, and individual stderr lines are limited to 2048
  characters. Job diagnostics are flushed after each write, including cancellation.

Use the matching video/batch IDs to follow hardware failures, CPU fallback, and
target-size retries. Compare snapshots with a faster job:

- Increasing frames with low fps and high CPU usage suggests compute-heavy work.
  CPU usage as a fraction of total CPU capacity is approximately the change in
  `cpu_total_s` divided by elapsed seconds and logical CPU count.
- Unchanged frames/output and increasing progress age suggest a stalled stage.
- High RAM load plus increasing page faults suggests possible memory pressure;
  page faults alone do not prove paging to disk.
- Read/write byte changes show process I/O activity, including cached I/O. They do
  not measure disk latency or prove that storage is the bottleneck.

These logs narrow down bottlenecks; they do not measure GPU utilization,
temperatures, CPU clock throttling, or per-stage decode/filter/encode timings.
OS termination or power loss can prevent a final record; earlier heartbeats remain
subject to normal log buffering. Logs contain local filenames/paths and media
metadata; review them before sharing. No media contents are uploaded by logging.
