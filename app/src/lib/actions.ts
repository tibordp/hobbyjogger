import { toast } from "sonner";

import type { Request } from "@/bindings/Request";
import { request } from "@/lib/backend";

/** Send a request, reporting a refusal as a toast. The core also logs it to the console. */
export async function act(r: Request): Promise<boolean> {
  try {
    await request(r);
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  }
}
