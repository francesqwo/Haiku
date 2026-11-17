fates-gfx-keys.txt: the native renderer's shaded (raster.frag) pipeline
states met while playing the replays (dev/smooth: the scene tour, the full
os-save3 replay, pressA and the cpu agent's save-state segments). At the
first 3D draw a low-priority thread makes each of them once through the
pipeline cache (source/gfx/src/cache.rs, warm_start), so a first use finds
it cached instead of compiling it. One key per line, 16 bytes in hex
(render/shaded.rs key_bytes); keys met in play are added to
%LOCALAPPDATA%\haiku\pipelines\fates-gfx-keys.txt.
