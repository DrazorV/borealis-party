/** The main screens a signed-in account reaches on a phone, by route. Admin is signed in, so /admin is included. */
export const MAIN_PAGES = [
  { route: "/characters", name: "Start" },
  { route: "/content", name: "Additional Content" },
  { route: "/content/sources", name: "Sources" },
  { route: "/more", name: "More" },
  { route: "/account", name: "Account" },
  { route: "/admin", name: "Admin" },
] as const;
