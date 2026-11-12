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
