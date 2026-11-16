import type { Transition, Variants } from "motion/react";

/** Snappy spring for cards, panels and anything that changes layout. */
export const spring: Transition = { type: "spring", stiffness: 420, damping: 36, mass: 0.9 };

/** Softer spring for larger moves, like the paste field gliding to the top. */
export const gentleSpring: Transition = { type: "spring", stiffness: 260, damping: 30 };

/** Content that arrives: fades in while rising a few pixels. */
export const riseIn: Variants = {
  hidden: { opacity: 0, y: 10, filter: "blur(4px)" },
  visible: { opacity: 1, y: 0, filter: "blur(0px)", transition: spring },
  exit: { opacity: 0, y: -6, filter: "blur(4px)", transition: { duration: 0.14, ease: "easeIn" } },
};
