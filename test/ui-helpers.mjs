/** Operate the visible themed choices rather than a hidden form field. */
export async function choose(control,value){const id=await control.getAttribute("aria-controls");await control.click();const list=control.page().locator(`[id=${JSON.stringify(id)}]`);if(typeof value==="object")await list.getByRole("option",{name:value.label,exact:true}).click();else await list.locator(`[role=option][data-value=${JSON.stringify(String(value))}]`).click();}
export async function switchApp(page,id){
  // The app rail lists every app on desktop; phones put the rest under More.
  const name=({files:"Files",contacts:"Contacts",calendar:"Calendar",notes:"Notes",tasks:"Tasks"})[id],link=page.getByRole("navigation",{name:"Applications",exact:true}).getByRole("link",{name,exact:true});
  if(await link.isVisible())await link.click();else{await page.getByRole("button",{name:"More apps",exact:true}).click();await page.getByRole("menu",{name:"More apps",exact:true}).getByRole("menuitem",{name,exact:true}).click();}
  await page.waitForURL(url=>url.pathname===(id==="files"?"/":"/"+id));
  // Optional apps briefly render a loading header before mounting the workspace.
  await page.locator(({files:".files-main",contacts:".contacts-main",calendar:".calendar-main",notes:".notes-list",tasks:".workspace-list"})[id]).waitFor();
}
export async function fileDetails(page,name){await page.getByRole("button",{name:`Actions for ${name}`,exact:true}).click();await page.getByRole("menuitem",{name:"Details",exact:true}).click();}
export async function itemAction(page,name,action){await page.getByRole("button",{name:`Actions for ${name}`,exact:true}).click();await page.getByRole("menuitem",{name:action,exact:true}).click();}
export async function calendarDate(page,date){await page.getByRole("button",{name:"Choose date",exact:true}).click();const picker=page.getByRole("dialog",{name:"Choose calendar date",exact:true});await choose(picker.getByRole("combobox",{name:"Choose year",exact:true}),Number(date.slice(0,4)));await choose(picker.getByRole("combobox",{name:"Choose month",exact:true}),Number(date.slice(5,7))-1);await picker.locator(`[role=gridcell][data-date="${date}"]`).click();}
export async function noteDetails(page){await page.getByRole("button",{name:"Note options",exact:true}).click();await page.getByRole("menuitem",{name:"Note details",exact:true}).click();return page.getByRole("dialog",{name:"Note details",exact:true});}
/** Calendar views are a segmented radio group (Day, Week, Month, Agenda). */
export async function calendarView(page,view){await page.getByRole("radiogroup",{name:"Calendar view",exact:true}).locator(`[role=radio][data-value=${JSON.stringify(view)}]`).click();}
/** Opens the spotlight search from the top bar (if needed) and returns its field. */
export async function openSearch(page){const field=page.getByRole("combobox",{name:"Search all apps",exact:true});if(!await field.isVisible())await page.getByRole("button",{name:"Open search",exact:true}).click();await field.waitFor();return field;}
/** Escape clears the query, then closes the palette. */
export async function closeSearch(page){const palette=page.locator("dialog.spotlight[open]");for(let i=0;i<2&&await palette.count();i++)await page.keyboard.press("Escape");await palette.waitFor({state:"detached"});}
