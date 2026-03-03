export type RotationMode = "random" | "sequential" | "shuffle";

export type Interval =
  | "five_minutes"
  | "fifteen_minutes"
  | "thirty_minutes"
  | "one_hour"
  | "four_hours"
  | "daily";

export type FitMode = "center" | "crop" | "fit" | "span" | "stretch" | "tile";

export type MinResolution = "none" | "hd720" | "full_hd1080" | "qhd1440" | "uhd4k";

export interface MonitorInfo {
  width: number;
  height: number;
  x: number;
  y: number;
  scale_factor: number;
}

export interface Settings {
  stash_url: string;
  api_key: string;
  query_filter: string;
  rotation_mode: RotationMode;
  interval: Interval;
  fit_mode: FitMode;
  min_resolution: MinResolution;
  per_monitor: boolean;
  wifi_only: boolean;
}

export const INTERVAL_LABELS: Record<Interval, string> = {
  five_minutes: "5 minutes",
  fifteen_minutes: "15 minutes",
  thirty_minutes: "30 minutes",
  one_hour: "1 hour",
  four_hours: "4 hours",
  daily: "Daily",
};

export const ROTATION_MODE_LABELS: Record<RotationMode, string> = {
  random: "Random",
  sequential: "Sequential",
  shuffle: "Shuffle (no repeat)",
};

export const MIN_RESOLUTION_LABELS: Record<MinResolution, string> = {
  none: "None",
  hd720: "720p (HD)",
  full_hd1080: "1080p (Full HD)",
  qhd1440: "1440p (QHD)",
  uhd4k: "4K (UHD)",
};

export const FIT_MODE_LABELS: Record<FitMode, string> = {
  center: "Center",
  crop: "Crop",
  fit: "Fit",
  span: "Span",
  stretch: "Stretch",
  tile: "Tile",
};
