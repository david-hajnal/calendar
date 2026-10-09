document.getElementById('consent').addEventListener('submit', async (event) => {
 event.preventDefault(); const form = event.currentTarget; const decision = event.submitter.value;
 const calendar_ids = Array.from(form.querySelectorAll('input:checked'), input => Number(input.value));
 const buttons = form.querySelectorAll('button'); buttons.forEach(button => button.disabled = true);
 try {
  const response = await fetch('/consent/decision', {method:'POST',credentials:'same-origin',headers:{'content-type':'application/json','x-csrf-token':form.dataset.csrf},body:JSON.stringify({handoff:form.dataset.handoff,decision,calendar_ids})});
  const result = await response.json(); if (!response.ok) throw new Error('Authorization failed. Please restart authorization.');
  window.location.assign(result.resume_url);
 } catch (error) { document.getElementById('error').textContent = error.message; buttons.forEach(button => button.disabled = false); }
});
