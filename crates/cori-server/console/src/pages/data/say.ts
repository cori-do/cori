import { toast } from "../../ui/toast";

/** A notice from the sheet: bad news as an error. */
export const say = (message: string, bad = false) => (bad ? toast.error(message) : toast(message));
