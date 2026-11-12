import { tx } from "./i18n";
export function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error(tx("That file couldn't be read.")));
    reader.readAsDataURL(file);
  });
}
