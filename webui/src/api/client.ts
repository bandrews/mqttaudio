// Typed client over a DaemonConnection. A thin, faithful mirror of API-CONTRACT:
// the full play surface goes via POST /command (DW10), the other commands via
// their typed routes (encoding the typed field names, e.g. voice_fade_out's
// `time_ms`). The client validates and WARNS — it never silently clamps or
// fabricates a selector.

import type { DaemonConnection } from './connection';
import type {
  CommandResponse,
  CacheStatus,
  FileParam,
  HealthInfo,
  InputMuteParams,
  InputVolumeParams,
  InputsResponse,
  MetricsInfo,
  PlayParams,
  SamplesResponse,
  SeekParams,
  SpeedParams,
  StatusInfo,
  StopParams,
  VersionInfo,
  VoiceFadeOutParams,
  VoiceStopParams,
  VoiceVolumeParams,
  VolumeParams,
  VoicesResponse,
  SampleSelector,
} from './contract';

/** Drop keys whose value is `undefined`, preserving the rest verbatim. */
function compact<T extends Record<string, unknown>>(obj: T): Partial<T> {
  const out: Partial<T> = {};
  for (const key of Object.keys(obj) as (keyof T)[]) {
    if (obj[key] !== undefined) {
      out[key] = obj[key];
    }
  }
  return out;
}

function isSelectorEmpty(sel: SampleSelector): boolean {
  return !sel.internal_id && !sel.id && !sel.file && !sel.voice;
}

export class DaemonClient {
  constructor(private readonly conn: DaemonConnection) {}

  // ---- Commands ----

  /** Play. Routed via /command so the full surface (channel_map/mode/...) reaches the daemon (DW10). */
  play(params: PlayParams): Promise<CommandResponse> {
    if (params.volume !== undefined && (params.volume < 0 || params.volume > 1)) {
      console.warn(`play: volume ${params.volume} is outside [0,1]; the daemon will clamp it.`);
    }
    return this.command('play', compact({ ...params }));
  }

  /** Send an arbitrary command verbatim (used by the raw editor and matrix mixer). */
  rawCommand(payload: Record<string, unknown>): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/command', payload);
  }

  stop(params: StopParams): Promise<CommandResponse> {
    this.warnEmptySelector('stop', params);
    return this.conn.post<CommandResponse>('/stop', compact({ ...params }));
  }

  stopAll(): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/stopall', {});
  }

  volume(params: VolumeParams): Promise<CommandResponse> {
    this.warnEmptySelector('volume', params);
    if (params.volume < 0 || params.volume > 1) {
      console.warn(`volume: ${params.volume} is outside [0,1]; the daemon will clamp it.`);
    }
    return this.conn.post<CommandResponse>('/volume', compact({ ...params }));
  }

  seek(params: SeekParams): Promise<CommandResponse> {
    this.warnEmptySelector('seek', params);
    return this.conn.post<CommandResponse>('/seek', compact({ ...params }));
  }

  speed(params: SpeedParams): Promise<CommandResponse> {
    this.warnEmptySelector('speed', params);
    const [lo, hi] = params.pitch_correction ? [0.05, 8.0] : [-100, 100];
    if (params.speed < lo || params.speed > hi) {
      console.warn(
        `speed: ${params.speed} is outside [${lo},${hi}] for pitch_correction=${!!params.pitch_correction}; the daemon will clamp it.`,
      );
    }
    return this.conn.post<CommandResponse>('/speed', compact({ ...params }));
  }

  voiceVolume(params: VoiceVolumeParams): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/voice/volume', { ...params });
  }

  /** Typed /voice/fade_out takes `time_ms` (the raw wire key is `time`). */
  voiceFadeOut(params: VoiceFadeOutParams): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/voice/fade_out', { ...params });
  }

  voiceStop(params: VoiceStopParams): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/voice/stop', { ...params });
  }

  inputVolume(params: InputVolumeParams): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/input/volume', { ...params });
  }

  inputMute(params: InputMuteParams): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/input/mute', { ...params });
  }

  precache(params: FileParam): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/precache', { ...params });
  }

  cacheClear(): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/cache/clear', {});
  }

  cacheInvalidate(params: FileParam): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/cache/invalidate', { ...params });
  }

  cacheReload(params: FileParam): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/cache/reload', { ...params });
  }

  /** Emit a nested {command, message} to /command (the universal raw path). */
  private command(command: string, message: Record<string, unknown>): Promise<CommandResponse> {
    return this.conn.post<CommandResponse>('/command', { command, message });
  }

  private warnEmptySelector(command: string, sel: SampleSelector): void {
    if (isSelectorEmpty(sel)) {
      console.warn(
        `${command}: no selector set (internal_id/id/file/voice) — the daemon will match nothing (a silent no-op).`,
      );
    }
  }

  // ---- Reads ----

  health(): Promise<HealthInfo> {
    return this.conn.get<HealthInfo>('/health');
  }

  version(): Promise<VersionInfo> {
    return this.conn.get<VersionInfo>('/version');
  }

  metrics(): Promise<MetricsInfo> {
    return this.conn.get<MetricsInfo>('/metrics');
  }

  status(): Promise<StatusInfo> {
    return this.conn.get<StatusInfo>('/status');
  }

  statusSamples(): Promise<SamplesResponse> {
    return this.conn.get<SamplesResponse>('/status/samples');
  }

  statusVoices(): Promise<VoicesResponse> {
    return this.conn.get<VoicesResponse>('/status/voices');
  }

  statusCache(): Promise<CacheStatus> {
    return this.conn.get<CacheStatus>('/status/cache');
  }

  statusInputs(): Promise<InputsResponse> {
    return this.conn.get<InputsResponse>('/status/inputs');
  }
}
