// View-only state shared between panels. Nothing here affects the machine by itself.
import { create } from "zustand";

interface Ui {
  /** Jog speed as a fraction of the axis max rate; also used for click-to-move. */
  jogSpeed: number;
  /** Whether clicking the bed view moves the head there. */
  clickToMove: boolean;
}

export const useUi = create<Ui>(() => ({
  jogSpeed: 0.25,
  clickToMove: false,
}));
