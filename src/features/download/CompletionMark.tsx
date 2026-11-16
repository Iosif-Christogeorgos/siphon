import { motion } from "motion/react";

const SPARKS = Array.from({ length: 8 }, (_, index) => (index / 8) * Math.PI * 2);

/** The little celebration when a download lands: a drawn check, a ring and a burst of sparks. */
export function CompletionMark() {
  return (
    <div className="relative grid size-7 shrink-0 place-items-center">
      <motion.span
        className="absolute inset-0 rounded-full border-2 border-success"
        initial={{ scale: 0.6, opacity: 0.7 }}
        animate={{ scale: 2, opacity: 0 }}
        transition={{ duration: 0.7, ease: [0.16, 1, 0.3, 1] }}
      />
      {SPARKS.map((angle) => (
        <motion.span
          key={angle}
          className="absolute size-1 rounded-full bg-success"
          initial={{ x: 0, y: 0, scale: 0, opacity: 1 }}
          animate={{ x: Math.cos(angle) * 22, y: Math.sin(angle) * 22, scale: [0, 1, 0], opacity: [1, 1, 0] }}
          transition={{ duration: 0.65, delay: 0.08, ease: [0.16, 1, 0.3, 1] }}
        />
      ))}
      <motion.span
        className="absolute inset-0 rounded-full bg-success"
        initial={{ scale: 0 }}
        animate={{ scale: 1 }}
        transition={{ type: "spring", stiffness: 500, damping: 22 }}
      />
      <svg viewBox="0 0 24 24" className="relative size-4 text-background" fill="none" aria-hidden="true">
        <motion.path
          d="M5.5 12.5l4.2 4.2 8.8-9.4"
          stroke="currentColor"
          strokeWidth="2.75"
          strokeLinecap="round"
          strokeLinejoin="round"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.32, delay: 0.16, ease: "easeOut" }}
        />
      </svg>
    </div>
  );
}
