/** What the trim UI can ask of a preview player, whatever is behind it. */
export interface PlayerHandle {
  play: () => void;
  pause: () => void;
  /** Jumps to a time in seconds and shows that frame. */
  seek: (seconds: number) => void;
}

/** What a preview player reports back. */
export interface PlayerEvents {
  /** The player can be controlled. */
  onReady: () => void;
  /** Playback position in seconds; called often while playing. */
  onTime: (seconds: number) => void;
  onPlaying: (playing: boolean) => void;
  /** This source can't be played; the caller should try the next one. */
  onError: () => void;
}
