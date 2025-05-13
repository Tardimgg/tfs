import { Routes } from '@angular/router';
import {FsComponent} from './components/fs/fs.component';
import {RegistrationComponent} from './components/registration/registration.component';

export const routes: Routes = [
  { path: 'registration', component: RegistrationComponent },
  { path: 'fs', component: FsComponent },
  { path: 'fs/**', component: FsComponent },
  { path: '**', component: FsComponent },
];
