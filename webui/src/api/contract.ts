// Code-verified mirror of the mqttaudio daemon surface (see docs/webui/API-CONTRACT.md).
// These types are deliberately faithful to the wire contract, quirks and all:
// internal_id/input are strings, mode/freshness are lowercase unions, and the
// full play surface (channel_map/mode/window_ms/...) is play-only and reaches the
// daemon via POST /command (DW10). Do not "clean up" the quirks here.

/** A channel reference: a numeric index, a numeric string, or a configured alias. */
export type ChannelRef = number | string;

/** One channel-map route. `gain` defaults to unity; ignored on `mode:stream` plays. */
export interface ChannelMapping {
  src: ChannelRef;
  dest: ChannelRef;
  gain?: number;
}

export type LoadMode = 'auto' | 'full' | 'stream';
export type Freshness = 'trusting' | 'dev' | 'pinned';

/** Params for `play`. Only `file` is required; the rest mirror PlayMessage. */
export interface PlayParams {
  file: string;
  id?: string;
  volume?: number;
  voice?: string;
  loop?: boolean;
  crossfade_ms?: number;
  fade_in?: number;
  start_position_ms?: number;
  channel_map?: ChannelMapping[];
  mode?: LoadMode;
  window_ms?: number;
  prebuffer_ms?: number;
  freshness?: Freshness;
  cacheable?: boolean;
}

/** The OR-logic sample selector. An empty selector matches nothing (a silent no-op). */
export interface SampleSelector {
  internal_id?: string;
  id?: string;
  file?: string;
  voice?: string;
}

export type StopParams = SampleSelector & { fade_out_ms?: number };
export type VolumeParams = SampleSelector & { volume: number };
export type SeekParams = SampleSelector & { position_ms: number };
export type SpeedParams = SampleSelector & { speed: number; pitch_correction?: boolean };

export interface VoiceVolumeParams {
  voice: string;
  volume: number;
}
/** Typed `/voice/fade_out` uses `time_ms`; the raw wire key is `time`. */
export interface VoiceFadeOutParams {
  voice: string;
  time_ms: number;
}
export interface VoiceStopParams {
  voice: string;
}
/** `input` is a string: a numeric index ("0") or a voice_id. */
export interface InputVolumeParams {
  input: string;
  volume: number;
}
export interface InputMuteParams {
  input: string;
  mute: boolean;
}
export interface FileParam {
  file: string;
}

/** Generic command-endpoint response (`/command` and the typed POST routes). */
export interface CommandResponse {
  success: boolean;
  message?: string;
  error?: string;
}

// ---- Read endpoints (§5) ----

export interface HealthInfo {
  status: string;
  service: string;
  version: string;
}

export interface VersionInfo {
  name: string;
  version: string;
  git_sha?: string;
}

export interface CacheSide {
  entries: number;
  size_bytes: number;
}

export interface StatusInfo {
  status: string;
  version: string;
  active_samples: number;
  active_inputs: number;
  active_voices: number;
  output_channels: number;
  clip_count: number;
  xruns: number;
  cache: { memory: CacheSide; disk: CacheSide };
}

/**
 * A row from `/status/samples`. NOTE: `position`, `position_ms`, and
 * `progress_percent` are hard-coded `0` until Sprint W6 lands live telemetry —
 * never present them as live before then.
 */
export interface SampleInfo {
  internal_id: string;
  id: string | null;
  voice: string;
  file: string;
  position: number;
  position_ms: number;
  total_frames: number;
  total_ms: number;
  sample_rate: number;
  volume: number;
  voice_volume: number;
  speed: number;
  loop_mode: boolean;
  progress_percent: number;
  /** Windowed/streamed (forward-only). Present from Sprint W6; absent on older daemons. */
  windowed?: boolean;
}

export interface SamplesResponse {
  samples: SampleInfo[];
}

/** Telemetry opt-in state (Sprint W6). */
export interface TelemetryInfo {
  enabled: boolean;
}

export interface VoiceInfo {
  id: string;
  sample_count: number;
  volume: number;
  ducking_multiplier: number;
}

export interface VoicesResponse {
  voices: VoiceInfo[];
}

export interface InputInfo {
  index: number;
  voice_id: string;
  volume: number;
  channels: number;
  muted: boolean;
}

export interface InputsResponse {
  inputs: InputInfo[];
}

export interface CacheSideDetailed extends CacheSide {
  size_mb: number;
}

export interface CacheStatus {
  memory: CacheSideDetailed;
  disk: CacheSideDetailed;
}

export interface MetricsCache {
  memory_bytes: number;
  memory_entries: number;
  memory_headroom_bytes: number | null;
  memory_cap_bytes: number | null;
  disk_bytes: number;
}

export interface MetricsInfo {
  uptime_seconds: number;
  clips: number;
  xruns: number;
  active_voices: number;
  active_samples: number;
  active_inputs: number;
  output_channels: number;
  cache: MetricsCache;
  ducking: Record<string, number>;
}

/** Legacy command aliases (only these three commands have aliases). */
export const COMMAND_ALIASES: Record<string, string> = {
  play: 'soundPlay',
  stopall: 'soundStopAll',
  precache: 'soundPrecache',
};
