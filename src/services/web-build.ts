/** Compile-time compatibility only, never browser authority or release trust. */
declare const __APERTURE_WEB_BUILD__: { ui_id: string; api_schema: number } | null;
export const WEB_BUILD = typeof __APERTURE_WEB_BUILD__ === "undefined" ? null : __APERTURE_WEB_BUILD__;
export const SCHEMA_HEADER = "X-Aperture-Api-Schema";
export const incompatible = () => ({ code: "E_WEB_API_INCOMPATIBLE", message: "UI/API incompatible; reload Aperture" });
