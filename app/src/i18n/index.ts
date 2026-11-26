import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "./en.json";
import fr from "./fr.json";

export const resources = { en: { translation: en }, fr: { translation: fr } } as const;

void i18n.use(initReactI18next).init({
  resources,
  lng: "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
  returnNull: false,
});

export default i18n;
