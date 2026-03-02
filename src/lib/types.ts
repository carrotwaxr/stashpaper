export type RotationMode = "random" | "sequential" | "shuffle";

export type Interval =
  | "five_minutes"
  | "fifteen_minutes"
  | "thirty_minutes"
  | "one_hour"
  | "four_hours"
  | "daily";

export type FitMode = "center" | "crop" | "fit" | "span" | "stretch" | "tile";

export interface Settings {
  stash_url: string;
  api_key: string;
  query_filter: string;
  rotation_mode: RotationMode;
  interval: Interval;
  fit_mode: FitMode;
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

export const FIT_MODE_LABELS: Record<FitMode, string> = {
  center: "Center",
  crop: "Crop",
  fit: "Fit",
  span: "Span",
  stretch: "Stretch",
  tile: "Tile",
};
