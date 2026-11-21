declare const WAYLINE_VERSION: string;

declare module '*.css' {}

declare module '\0wayline-styles' {
  export const sheets: string[];
  export const add: (css: string) => void;
}
