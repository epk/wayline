declare const WAYLINE_VERSION: string;

declare module '*.css' {}

declare module '\0wayline-styles' {
  export const add: (css: string) => void;
}

declare module '\0wayline-manifest' {
  export const manifest: {
    base: string;
    core: string;
    entries: Record<string, string>;
    deps: Record<string, string[]>;
  };
}
